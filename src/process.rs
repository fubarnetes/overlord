use std::process;
use std::time;
use std::sync::{Mutex,Arc,mpsc};
use std::thread;
use std::io::{BufRead, BufReader, BufWriter, Write};

#[derive(Debug, PartialEq)]
pub enum State {
    Stopped,
    Starting,
    Running,
    Restarting,
    Failed,
}

#[derive(Debug)]
pub struct _Process {
    handle: Option<thread::JoinHandle<()>>,
    pub name: String,
    pub path: String,
    pub args: Vec<String>,
    pub restart_delay: u64,
    pub cwd: Option<String>,
    pub state: State,
    pub exit_status: Option<i32>,
    pub restart_count: u64,
    pub max_restart_count: u64,
    pub pid: Option<u32>,

    // stdio MPSC channels
    pub stdin: mpsc::Sender<String>,
    pub stdout: mpsc::Receiver<String>,
    pub stderr: mpsc::Receiver<String>,
    stdin_receiver: mpsc::Receiver<String>,
    stdout_sender: mpsc::Sender<String>,
    stderr_sender: mpsc::Sender<String>,

    // stdio BufWriters and BufReaders
    stdin_writer: Option<BufWriter<process::ChildStdin>>,
    stdout_reader: Option<BufReader<process::ChildStdout>>,
    stderr_reader: Option<BufReader<process::ChildStderr>>,

    child: Option<Arc<Mutex<process::Child>>>,
}

pub type Process = Arc<Mutex<_Process>>;

pub trait Runnable {
    fn define_process(name: &str, path: &str, args: Vec<String>,
       restart_delay: Option<u64>, cwd: Option<String>) -> Self;
    fn launch(self);
}

#[allow(unused_macros)]
macro_rules! from_argv {
    ( $argv:expr ) => {{
        let _argv = $argv.iter().map(|s| s.to_string()).collect();
        <Process as Runnable>::define_process($argv[0], $argv[0], _argv, None, None)
    }};
    ( $argv:expr, $cwd:expr ) => {{
        let _argv = $argv.iter().map(|s| s.to_string()).collect();
        <Process as Runnable>::define_process($argv[0], $argv[0], _argv,
                     None,
                     Some($cwd.to_string()))
    }};
    ( $argv:expr, $cwd:expr, $restart_delay:expr ) => {{
        let _argv = $argv.iter().map(|s| s.to_string()).collect();
        <Process as Runnable>::define_process($argv[0], $argv[0], _argv,
                     Some($restart_delay),
                     Some($cwd.to_string()))
    }};
}

impl Runnable for Process {
    fn define_process(name: &str, path: &str, args: Vec<String>,
           restart_delay: Option<u64>, cwd: Option<String>) -> Process {

        // set up stdio channels
        let (stdin, stdin_receiver) = mpsc::channel();
        let (stdout_sender, stdout) = mpsc::channel();
        let (stderr_sender, stderr) = mpsc::channel();

        return Arc::new(Mutex::new(_Process {
            handle: None,
            name: name.to_string(),
            path: path.to_string(),
            args: args,
            restart_delay: restart_delay.unwrap_or(0),
            cwd: cwd,
            state: State::Stopped,
            exit_status: None,
            restart_count: 0,
            max_restart_count: 5, // FIXME: this should be configurable
            pid: None,

            stdin: stdin,
            stdout: stdout,
            stderr: stderr,
            stdin_receiver: stdin_receiver,
            stdout_sender: stdout_sender,
            stderr_sender: stderr_sender,

            stdin_writer: None,
            stdout_reader: None,
            stderr_reader: None,

            child: None,
        }))
    }

    /// Launches the process.
    fn launch(self) {
        let lockable = self.clone();
        let handle = Some(thread::Builder::new()
            .name("overlord".to_string())
            .spawn(move || {
                loop {
                    // Run the process
                    let child = {
                        let mut p = lockable.lock().unwrap();
                        let mut cmd = process::Command::new(&p.path);
                        cmd.args(&p.args[1..]);

                        // If a working directory is specified, set it.
                        if p.cwd.is_some() {
                            cmd.current_dir(p.cwd.as_ref().unwrap());
                        }

                        // Set up stdin, stdout, and stderr
                        cmd.stdin(process::Stdio::piped());
                        cmd.stdout(process::Stdio::piped());
                        cmd.stderr(process::Stdio::piped());

                        // Spawn the child
                        let child = Arc::new(Mutex::new(cmd.spawn().expect("Failed to run binary")));

                        // Set up BufReaders and BufWriters for stdin, stdout and stderr
                        p.stdin_writer = Some(BufWriter::new(child.lock().unwrap().stdin.take().unwrap()));
                        p.stdout_reader = Some(BufReader::new(child.lock().unwrap().stdout.take().unwrap()));
                        p.stderr_reader = Some(BufReader::new(child.lock().unwrap().stderr.take().unwrap()));

                        p.state = State::Running;
                        p.pid = Some(child.lock().unwrap().id());
                        p.child = Some(child.clone());
                        child
                    };

                    // Process supervisor main loop
                    let exit_status = loop {
                        thread::sleep(time::Duration::from_millis(10));

                        let mut p = lockable.lock().unwrap();
                        // p is a MutexGuard<_Process>, so each access to fields of _Process calls
                        // deref / deref_mut to get the underlying _Process. To avoid borrowing
                        // conflicts, get a reference to the underlying _Process struct once.
                        let p = &mut *p;

                        // Handle stdio
                        if let Some(ref mut stdout) = p.stdout_reader {
                            for line in stdout.lines() {
                                p.stdout_sender
                                    .send(line.unwrap())
                                    .expect("Could not send stdout");
                            }
                        }

                        if let Some(ref mut stderr) = p.stderr_reader {
                            for line in stderr.lines() {
                                p.stderr_sender
                                    .send(line.unwrap())
                                    .expect("Could not send stderr");
                            }
                        }

                        if let Some(ref mut stdin) = p.stdin_writer {
                            if let Ok(input) = p.stdin_receiver.try_recv() {
                                info!("received: {}", input);
                                stdin.write_all(input.as_bytes())
                                    .expect("Could not write to stdin");
                            }
                        }

                        // Check if the process has already exited
                        //
                        // Note: we can't really use child.lock().unwrap().wait() here as that would
                        //   - close stdin
                        //   - require a mutable copy of child, and therefore make it necessary to
                        //     lock its Mutex, rendering it impossible to call e.g. kill() on from
                        //     another thread.
                        //
                        // The shared_child crate is also unsuitable as it has quite a few shortcomings
                        // (e.g. still using a Mutex as opposed to a RwLock) and it's really not that
                        // critical that we're fast here.
                        match child.lock().unwrap().try_wait() {
                            Ok(Some(status)) => {
                                p.exit_status = if status.code().is_some() {
                                    Some(status.code().expect("Could not get exit status"))
                                } else {
                                    error!("Killed by Signal");
                                    None
                                };
                                break Ok(p.exit_status);
                            }
                            Err(e) => {
                                break Err(e);
                            }
                            Ok(None) => {
                                continue;
                            }
                        }
                    };
                    info!("exit code {:?}", exit_status);

                    // Depending on the exit status, restart or fail the process
                    match exit_status {
                        Ok(Some(0)) => {
                            info!("Exited with 0. Restarting...");
                        }
                        Ok(Some(_)) | Ok(None)  => {
                            info!("Failed. Restarting...");
                        }
                        Err(e) => {
                            error!("Error: {}. Not restarting...", e);
                            let mut p = lockable.lock().unwrap();
                            p.state = State::Failed;
                            break;
                        }
                    };

                    let restart_delay = {
                        let mut p = lockable.lock().unwrap();

                        // Do not restart more than p.max_restart_count times.
                        if p.restart_count >= p.max_restart_count {
                            error!("Restarted to often. Not restarting...");
                            p.state = State::Failed;
                            break;
                        }

                        p.restart_count += 1;
                        p.state = State::Restarting;
                        p.restart_delay
                    };

                    thread::sleep(time::Duration::from_millis(restart_delay));
                }
            }).expect("Failed to spawn process"));
            self.lock().unwrap().handle = handle;
    }
}


#[cfg(test)]
mod tests {
    use std::time;
    use super::*;
    extern crate pretty_env_logger;

    macro_rules! sleep {
        ( $ms:expr ) => ({
            let duration = time::Duration::from_millis($ms);
            thread::sleep(duration);
        })
    }

    #[test]
    fn test_run_ls_max_retries() {
        let interval = 1000;
        let p = from_argv!(["ls", "-la"], "/", interval);

        p.clone().launch();

        sleep!(interval*p.lock().unwrap().max_restart_count);
        sleep!(interval);

        info!("{:?}",p);
    }

    #[test]
    fn test_exitstatus() {
        let p = from_argv!(["false"]);
        p.lock().unwrap().max_restart_count = 0;
        p.clone().launch();
        sleep!(500);
        assert_eq!(p.lock().unwrap().exit_status, Some(1));
    }

    #[test]
    fn test_echo() {
        let p = from_argv!(["echo", "test"], "/");
        p.lock().unwrap().max_restart_count = 0;
        p.clone().launch();
        sleep!(500);
        assert_eq!(p.lock().unwrap().stdout.recv(), Ok("test".to_string()));
    }
}

use std::process;
use std::time;
use std::sync::{Mutex,Arc};
use std::thread;

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
                        if p.cwd.is_some() {
                            cmd.current_dir(p.cwd.as_ref().unwrap());
                        }
                        let child = Arc::new(Mutex::new(cmd.spawn().expect("Failed to run binary")));

                        p.state = State::Running;
                        p.pid = Some(child.lock().unwrap().id());
                        p.child = Some(child.clone());
                        child
                    };

                    // Wait for the process to exit
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
                    let exit_status = loop {
                        thread::sleep(time::Duration::from_millis(10));
                        match child.lock().unwrap().try_wait() {
                            Ok(Some(status)) => {
                                if status.code().is_some() {
                                    let mut p = lockable.lock().unwrap();
                                    p.exit_status = Some(status.code().expect("Could not get exit status"));
                                    break Ok(p.exit_status);
                                } else {
                                    error!("Killed by Signal");
                                    break Ok(None);
                                }
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

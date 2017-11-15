use std::process;
use std::time;
use std::sync::{Mutex,Arc};
use std::thread;

#[derive(Debug)]
pub enum State {
    Stopped,
    Starting,
    Running,
    Restarting
}

#[derive(Debug)]
pub struct ProcessLockable {
    handle: Option<thread::JoinHandle<()>>,
    pub name: String,
    pub path: String,
    pub args: Vec<String>,
    pub restart_delay: u64,
    pub cwd: Option<String>,
    pub state: State,
    pub exit_status: Option<u32>,
    pub restart_count: u64,
}

#[derive(Debug)]
pub struct Process {
    lockable: Arc<Mutex<ProcessLockable>>,
}


#[allow(unused_macros)]
macro_rules! from_argv {
    ( $argv:expr ) => {{
        let _argv = $argv.iter().map(|s| s.to_string()).collect();
        Process::new($argv[0], $argv[0], _argv, None, None)
    }};
    ( $argv:expr, $cwd:expr ) => {{
        let _argv = $argv.iter().map(|s| s.to_string()).collect();
        Process::new($argv[0], $argv[0], _argv,
                     None,
                     Some($cwd.to_string()))
    }};
    ( $argv:expr, $cwd:expr, $restart_delay:expr ) => {{
        let _argv = $argv.iter().map(|s| s.to_string()).collect();
        Process::new($argv[0], $argv[0], _argv,
                     Some($restart_delay),
                     Some($cwd.to_string()))
    }};
}

impl Process {
    pub fn new(name: &str, path: &str, args: Vec<String>,
           restart_delay: Option<u64>, cwd: Option<String>) -> Process {
        return Process {
            lockable:Arc::new(Mutex::new(ProcessLockable {
                handle: None,
                name: name.to_string(),
                path: path.to_string(),
                args: args,
                restart_delay: restart_delay.unwrap_or(0),
                cwd: cwd,
                state: State::Stopped,
                exit_status: None,
                restart_count: 0,
            }))
        }
    }

    pub fn launch(&mut self) {
        let lockable = self.lockable.clone();
        self.lockable.lock().unwrap().handle = Some(thread::Builder::new()
            .name("overlord".to_string())
            .spawn(move || {
                loop {
                    let mut child = {
                        let mut p = lockable.lock().unwrap();
                        let mut cmd = process::Command::new(&p.path);
                        cmd.args(&p.args);
                        if p.cwd.is_some() {
                            cmd.current_dir(p.cwd.as_ref().unwrap());
                        }
                        let child = cmd.spawn().expect("Failed to run binary");
                        p.state = State::Running;
                        child
                    };

                    let exit = child.wait();
                    info!("exit code{:?}", exit);
                    {
                        let mut p = lockable.lock().unwrap();
                        /*
                        let exit_status = match exit {
                            Ok(status) => {
                                if status.code.is_some() {
                                    Ok(Some(status.code))
                                } else {
                                    error!("Killed by Signal");
                                    Ok(None)
                                }
                            }
                            Err(e) => {
                                Err(e)
                            }
                        };

                        match exit_status {
                            Ok(Some(0)) => {
                                info!("Exited. Restarting...");
                            }
                            Ok(Some(status)) | Ok(None)  => {
                                info!("Failed. Restarting...");
                            }
                            Err(e) => {
                                error!("Error. Not Restarting...");
                                break;
                            }
                        }; */

                        thread::sleep(time::Duration::from_millis(p.restart_delay));
                    }

                }
            }).expect("Failed to spawn process"))
    }
}

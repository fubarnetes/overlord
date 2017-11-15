#![crate_type="lib"]
#![crate_name = "overlord"]
#![allow(dead_code)]

#[macro_use]
extern crate log;

use std::sync::{Mutex,Arc,mpsc};
use std::thread;

#[macro_use]
mod process;

use process::Process;
use process::Runnable;

#[derive(Debug)]
enum Command {
    Spawn(Process),
    Quit,
}

#[derive(Debug)]
pub struct Overlord {
    /* Channel to push new commands to the list */
    channel: mpsc::Sender<Command>,
    handle: thread::JoinHandle<()>,
    processes: SharedProcessList,
}

type ProcessList = Vec<Process>;
type SharedProcessList = Arc<Mutex<ProcessList>>;

impl Overlord {
    pub fn new() -> Overlord {
        let (tx, rx) : (_, mpsc::Receiver<Command>)= mpsc::channel();

        let processes: SharedProcessList = Arc::new(Mutex::new(Vec::new()));

        let handle = {
            let processes = processes.clone();
            thread::Builder::new()
                .name("overlord".to_string())
                .spawn(move || {
                loop {
                    match rx.recv().expect("received None on channel") {
                        Command::Spawn(p) => {
                            trace!("Pushed {:?}", p);
                            let mut plist = processes.lock().unwrap();
                            plist.push(p.clone());
                            p.launch();
                        }
                        Command::Quit => {
                            trace!("Terminating");
                            break
                        }
                    }
                }
            }).expect("Failed to spawn Overlord")
        };

        return Overlord{
            channel: tx,
            handle,
            processes,
        };
    }

    pub fn quit(self) {
        self.channel.send(Command::Quit).unwrap();
        self.handle.join().unwrap();
    }

    pub fn spawn(&self, process: Process) {
        self.channel.send(Command::Spawn(process)).unwrap();
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
        let _ = pretty_env_logger::init();
        let instance = Overlord::new();

        {
            let processes = instance.processes.lock().unwrap();
            trace!("No processes should exist at this point.");
            assert_eq!(processes.len(), 0);
        }

        // Run ls every second
        let interval = 1000;
        let p = from_argv!(["ls", "-la"], "/", interval);

        info!("Created {:?}", p);

        instance.spawn(p);

        // Sleep a bit to allow the overlord thread to handle stuff.
        sleep!(interval/2);

        // At this point the process should have restarted exactly once
        {
            let processes = instance.processes.lock().unwrap();
            assert_eq!(processes.len(), 1);

            let p = &processes[0].lock().unwrap();
            assert_eq!(p.name, "ls");
            assert_eq!(p.state, process::State::Restarting);
            assert_eq!(p.exit_status, Some(0));
            assert_eq!(p.restart_count, 1);
        }

        sleep!({
                let processes = instance.processes.lock().unwrap();
                let p = &processes[0].lock().unwrap();
                p.max_restart_count
            }*interval);

        {
            let processes = instance.processes.lock().unwrap();
            assert_eq!(processes.len(), 1);

            let p = &processes[0].lock().unwrap();
            assert_eq!(p.name, "ls");
            assert_eq!(p.state, process::State::Failed);
            assert_eq!(p.exit_status, Some(0));
            assert_eq!(p.restart_count, p.max_restart_count);
        }

        instance.quit();
    }
}

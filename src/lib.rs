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

type ProcessList = Vec<Arc<Process>>;
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
                            plist.push(Arc::new(p));
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
    fn test_run() {
        let _ = pretty_env_logger::init();
        let instance = Overlord::new();

        {
            let processes = instance.processes.lock().unwrap();
            trace!("No processes should exist at this point.");
            assert_eq!(processes.len(), 0);
        }

        let p = from_argv!(["ls", "-la"], "/");

        instance.spawn(p);
        // Sleep a bit to allow the overlord thread to handle stuff
        sleep!(10);

        {
            let processes = instance.processes.lock().unwrap();
            info!("the following processes exist: {:?}", *processes);
            assert_eq!(processes.len(), 1);
        }

        instance.quit();
    }
}

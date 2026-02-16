use std::error::Error;
use std::io::{BufRead, BufReader, Write, Read};
use std::path::{Path};
use std::process::{Command, Stdio, Child, ChildStdin};
use std::time::Duration;
use crossbeam_channel::Receiver;

pub struct SailRepl {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    rx: Option<Receiver<String>>,
}

impl SailRepl {
    pub fn new() -> Self {
        Self { child: None, stdin: None, rx: None }
    }

    pub fn spawn(&mut self, sail_path: &str, file_to_load: &Path) -> Result<Vec<String>, Box<dyn Error + Send + Sync>> {
        if let Some(mut old) = self.child.take() {
            let _ = old.kill();
            let _ = old.wait();
        }

        let mut cmd = Command::new(sail_path);
        cmd.arg("-i").arg("--no-color");
        cmd.arg(file_to_load);
        
        let mut child = cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or("Failed to open stdout")?;
        let stderr = child.stderr.take().ok_or("Failed to open stderr")?;
        
        let (tx, rx) = crossbeam_channel::unbounded();

        let tx_out = tx.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut buf: Vec<u8> = Vec::new();
            let mut byte = [0u8; 1];
            while reader.read_exact(&mut byte).is_ok() {
                buf.push(byte[0]);
                if byte[0] == b'\n' || buf.ends_with(b"Sail REPL> ") {
                    let s = String::from_utf8_lossy(&buf).into_owned();
                    let _ = tx_out.send(s);
                    buf.clear();
                }
            }
        });

        let tx_err = tx;
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if let Ok(l) = line {
                    let _ = tx_err.send(format!("STDERR:{}", l));
                }
            }
        });

        self.child = Some(child);
        self.stdin = stdin;
        self.rx = Some(rx);
        
        Ok(self.wait_for_prompt(Duration::from_secs(30)))
    }

    pub fn wait_for_prompt(&mut self, timeout: Duration) -> Vec<String> {
        let mut lines = Vec::new();
        let start = std::time::Instant::now();
        if let Some(rx) = &self.rx {
            while start.elapsed() < timeout {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(line) => {
                        if line.contains("Sail REPL>") { break; }
                        lines.push(line);
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                }
            }
        }
        lines
    }

    pub fn send_command(&mut self, cmd: &str) -> Vec<String> {
        if let Some(stdin) = &mut self.stdin {
            if writeln!(stdin, "{}", cmd).is_err() { return vec![]; }
            let _ = stdin.flush();
        }
        self.wait_for_prompt(Duration::from_secs(5))
    }

    pub fn is_alive(&mut self) -> bool {
        if let Some(child) = &mut self.child {
            child.try_wait().map(|s| s.is_none()).unwrap_or(false)
        } else {
            false
        }
    }
}

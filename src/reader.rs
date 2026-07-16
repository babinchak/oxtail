use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use flate2::read::GzDecoder;

/// Spawn the reader thread. Lines arrive on the returned channel; the sender
/// hanging up means EOF (or the source errored).
pub fn spawn(path: Option<PathBuf>, follow: bool, rate: Option<u32>) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = read_source(path, follow, rate, tx);
    });
    rx
}

fn read_source(
    path: Option<PathBuf>,
    follow: bool,
    rate: Option<u32>,
    tx: Sender<String>,
) -> io::Result<()> {
    // Pacing for demo replay: send `batch` lines, then sleep `delay`. Keeps
    // sleeps coarse enough for Windows timer granularity at high rates.
    let pace = rate.map(|r| {
        let r = r.max(1);
        if r <= 100 {
            (1u32, Duration::from_millis(1000 / r as u64))
        } else {
            (r / 100, Duration::from_millis(10))
        }
    });
    let mut sent_in_batch = 0u32;
    let mut first = true;
    let mut send = |line: &str| -> bool {
        // PowerShell pipes and many Windows editors prepend a UTF-8 BOM,
        // which would make the first line unparseable as JSON.
        let line = if std::mem::take(&mut first) {
            line.trim_start_matches('\u{feff}')
        } else {
            line
        };
        let line = line.trim_end_matches(['\r', '\n']);
        if tx.send(line.to_string()).is_err() {
            return false; // UI quit; stop reading
        }
        if let Some((batch, delay)) = pace {
            sent_in_batch += 1;
            if sent_in_batch >= batch {
                sent_in_batch = 0;
                thread::sleep(delay);
            }
        }
        true
    };

    match path {
        None => {
            for line in io::stdin().lock().lines() {
                if !send(&line?) {
                    return Ok(());
                }
            }
        }
        Some(path) => {
            let is_gz = path.extension().is_some_and(|e| e == "gz");
            let file = File::open(&path)?;
            let mut reader: Box<dyn BufRead> = if is_gz {
                Box::new(BufReader::new(GzDecoder::new(file)))
            } else {
                Box::new(BufReader::new(file))
            };

            let mut buf = String::new();
            loop {
                buf.clear();
                let n = reader.read_line(&mut buf)?;
                if n == 0 {
                    // EOF: keep polling in follow mode (plain files only),
                    // otherwise we're done.
                    if follow && !is_gz {
                        thread::sleep(Duration::from_millis(200));
                        continue;
                    }
                    return Ok(());
                }
                if !send(&buf) {
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

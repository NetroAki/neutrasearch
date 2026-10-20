use super::*;

pub(super) struct Capture {
    text: Arc<Mutex<String>>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl Capture {
    pub(super) fn start(stderr: Option<std::process::ChildStderr>) -> Self {
        let text = Arc::new(Mutex::new(String::new()));
        let reader = stderr.map(|stderr| {
            let captured = Arc::clone(&text);
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    eprintln!("neutrasearch-helper: {line}");
                    if let Ok(mut text) = captured.lock() {
                        if text.len() < 8192 {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(&line);
                            while text.len() > 8192 {
                                text.pop();
                            }
                        }
                    }
                }
            })
        });
        Self { text, reader }
    }

    pub(super) fn finish(self) -> Arc<Mutex<String>> {
        if let Some(reader) = self.reader {
            let _ = reader.join();
        }
        self.text
    }
}

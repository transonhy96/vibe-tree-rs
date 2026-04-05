use crate::session::{PtyError, PtySession};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use tracing;

/// Event sent from PTY sessions to the app.
#[derive(Debug, Clone)]
pub enum PtyEvent {
    Output { session_id: u64, data: Vec<u8> },
    Exited { session_id: u64, code: i32 },
}

struct SessionEntry {
    session: PtySession,
    output_tx: broadcast::Sender<Vec<u8>>,
}

pub struct SessionManager {
    sessions: Arc<Mutex<HashMap<u64, SessionEntry>>>,
    next_id: AtomicU64,
    event_tx: tokio::sync::mpsc::UnboundedSender<PtyEvent>,
}

impl SessionManager {
    pub fn new(event_tx: tokio::sync::mpsc::UnboundedSender<PtyEvent>) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            event_tx,
        }
    }

    pub async fn spawn_session(
        &self,
        worktree_path: &Path,
        cols: u16,
        rows: u16,
    ) -> Result<u64, PtyError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut session = PtySession::spawn(id, worktree_path, cols, rows)?;

        let reader = session
            .take_reader()
            .ok_or_else(|| PtyError::OpenFailed("no reader available".into()))?;

        let (output_tx, _) = broadcast::channel(256);

        let entry = SessionEntry {
            session,
            output_tx: output_tx.clone(),
        };

        self.sessions.lock().await.insert(id, entry);

        // Spawn async reader task
        let event_tx = self.event_tx.clone();
        let sessions = self.sessions.clone();
        tokio::task::spawn_blocking(move || {
            Self::read_loop(id, reader, output_tx, event_tx, sessions);
        });

        tracing::info!(session_id = id, "PTY session spawned");
        Ok(id)
    }

    fn read_loop(
        session_id: u64,
        mut reader: Box<dyn Read + Send>,
        output_tx: broadcast::Sender<Vec<u8>>,
        event_tx: tokio::sync::mpsc::UnboundedSender<PtyEvent>,
        sessions: Arc<Mutex<HashMap<u64, SessionEntry>>>,
    ) {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let data = buf[..n].to_vec();
                    let _ = output_tx.send(data.clone());
                    let _ = event_tx.send(PtyEvent::Output {
                        session_id,
                        data,
                    });
                }
                Err(_) => break,
            }
        }

        // Session ended - check exit code
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async {
            let mut sessions = sessions.lock().await;
            let code = sessions
                .get_mut(&session_id)
                .and_then(|e| e.session.try_wait())
                .map(|c| c as i32)
                .unwrap_or(-1);

            sessions.remove(&session_id);
            let _ = event_tx.send(PtyEvent::Exited { session_id, code });
            tracing::info!(session_id, code, "PTY session exited");
        });
    }

    pub async fn write(&self, session_id: u64, data: &[u8]) -> Result<(), PtyError> {
        let mut sessions = self.sessions.lock().await;
        let entry = sessions
            .get_mut(&session_id)
            .ok_or(PtyError::NotFound(session_id))?;
        entry.session.write(data)
    }

    pub async fn resize(&self, session_id: u64, cols: u16, rows: u16) -> Result<(), PtyError> {
        let sessions = self.sessions.lock().await;
        let entry = sessions
            .get(&session_id)
            .ok_or(PtyError::NotFound(session_id))?;
        entry.session.resize(cols, rows)
    }

    pub async fn kill(&self, session_id: u64) -> Result<(), PtyError> {
        let mut sessions = self.sessions.lock().await;
        if let Some(entry) = sessions.get_mut(&session_id) {
            entry.session.kill();
            sessions.remove(&session_id);
            Ok(())
        } else {
            Err(PtyError::NotFound(session_id))
        }
    }

    pub async fn subscribe_output(&self, session_id: u64) -> Option<broadcast::Receiver<Vec<u8>>> {
        let sessions = self.sessions.lock().await;
        sessions.get(&session_id).map(|e| e.output_tx.subscribe())
    }

    pub async fn session_count(&self) -> usize {
        self.sessions.lock().await.len()
    }

    pub async fn kill_all(&self) {
        let mut sessions = self.sessions.lock().await;
        for (_, entry) in sessions.iter_mut() {
            entry.session.kill();
        }
        sessions.clear();
    }
}

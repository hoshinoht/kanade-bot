//! The outside world the server talks to. Discord's authorization page is a
//! browser page, so it lives on the harness's own origins; the scouter, the
//! NEXON Open API and the model gateway are only called by the server, so
//! each is a loopback server on an ephemeral port. The gateway's guild cache
//! is [`guild::Guild`], installed in each composition.

pub mod discord;
pub mod gateway;
pub mod guild;
mod nexon;
mod scouter;

use std::{io, net::SocketAddr, sync::Arc};

use axum::Router;
use tokio::{net::TcpListener, task::JoinHandle};

/// The running fakes; they stop when this is dropped.
pub struct Fakes {
    pub scouter: SocketAddr,
    pub nexon: SocketAddr,
    pub gateway: SocketAddr,
    /// The model gateway's completions.
    pub model: Arc<gateway::Activity>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Fakes {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

pub async fn start() -> io::Result<Fakes> {
    let mut tasks = Vec::with_capacity(3);
    let model = Arc::new(gateway::Activity::default());
    Ok(Fakes {
        scouter: spawn(scouter::router(), &mut tasks).await?,
        nexon: spawn(nexon::router(), &mut tasks).await?,
        gateway: spawn(gateway::router(Arc::clone(&model)), &mut tasks).await?,
        model,
        tasks,
    })
}

async fn spawn(app: Router, tasks: &mut Vec<JoinHandle<()>>) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind((crate::LOOPBACK, 0)).await?;
    let address = listener.local_addr()?;
    tasks.push(tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    }));
    Ok(address)
}

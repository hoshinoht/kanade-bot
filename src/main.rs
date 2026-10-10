use std::process::ExitCode;

use kanade::runtime::{application, logging, tls};

#[tokio::main]
async fn main() -> ExitCode {
    let result = tls::install_ring_provider().map(|()| {
        (
            std::env::args().skip(1).collect(),
            std::env::vars().collect(),
        )
    });
    let result = match result {
        Ok((arguments, environment)) => application::run(arguments, environment).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            logging::error(&error);
            ExitCode::from(error.exit_code())
        }
    }
}

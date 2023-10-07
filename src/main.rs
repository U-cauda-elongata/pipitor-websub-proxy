mod run;
mod util;

use std::env;
use std::ffi::OsStr;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use listenfd::ListenFd;
use tokio::net::UnixListener;

const DEFAULT_CLIENT_SOCK: &str = "/run/pipitor.sock";

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    tracing_subscriber::fmt::init();
    let (client_sock, incoming) = match process_args()? {
        ControlFlow::Continue(ret) => ret,
        ControlFlow::Break(code) => return Ok(code),
    };
    run::main(client_sock, incoming).await?;
    Ok(ExitCode::SUCCESS)
}

fn process_args() -> anyhow::Result<ControlFlow<ExitCode, (PathBuf, UnixListener)>> {
    let mut args = env::args_os();
    let prog = args.next();
    let client = args.next();
    let (client_sock, server_sock) = if let Some(ref client) = client {
        (client.into(), args.next())
    } else if Path::new(DEFAULT_CLIENT_SOCK).exists() {
        (DEFAULT_CLIENT_SOCK.into(), None)
    } else {
        eprintln!(
            "Missing SOCKET_PATH argument and the default path {DEFAULT_CLIENT_SOCK:?} not found"
        );
        print_usage(prog.as_deref());
        return Ok(ControlFlow::Break(ExitCode::FAILURE));
    };

    let incoming = if let Some(ref path) = server_sock {
        UnixListener::bind(path)?
    } else if let Some(incoming) = ListenFd::from_env().take_unix_listener(0)? {
        UnixListener::from_std(incoming)?
    } else {
        eprintln!("Either BIND argument or `LISTEN_FD` environment variable must be provided");
        print_usage(prog.as_deref());
        return Ok(ControlFlow::Break(ExitCode::FAILURE));
    };

    Ok(ControlFlow::Continue((client_sock, incoming)))
}

fn print_usage(prog: Option<&OsStr>) {
    let prog = prog.unwrap_or(env!("CARGO_PKG_NAME").as_ref());
    eprintln!("usage: {prog:?} [CLIENT_SOCKET [BIND]]");
}

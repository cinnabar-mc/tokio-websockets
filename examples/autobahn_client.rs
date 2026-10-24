use std::{io, str::FromStr, time::Duration};

use futures_util::{SinkExt, StreamExt};
use http::Uri;
use tokio::time::{Instant, sleep_until, timeout_at};
use tokio_websockets::{ClientBuilder, Connector, Error, Limits};

const SERVER: &str = "ws://127.0.0.1:9001";

async fn get_case_count() -> Result<u32, Error> {
    let uri = Uri::from_str(&format!("{SERVER}/getCaseCount")).unwrap();
    let (mut stream, _) = ClientBuilder::from_uri(uri)
        .connector(&Connector::Plain)
        .connect()
        .await?;
    let msg = stream.next().await.ok_or_else(|| {
        io::Error::new(io::ErrorKind::UnexpectedEof, "missing Autobahn case count")
    })??;

    stream.close().await?;

    let count = msg.as_text().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Autobahn case count is not text",
        )
    })?;
    count
        .parse::<u32>()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error).into())
}

async fn update_reports() -> Result<(), Error> {
    let uri = Uri::from_str(&format!("{SERVER}/updateReports?agent=tokio-websockets")).unwrap();
    let (mut stream, _) = ClientBuilder::from_uri(uri)
        .connector(&Connector::Plain)
        .connect()
        .await?;

    stream.close().await?;

    Ok(())
}

async fn run_test(case: u32) -> Result<(), Error> {
    println!("Running test case {case}");

    let uri = Uri::from_str(&format!(
        "{SERVER}/runCase?case={case}&agent=tokio-websockets",
    ))
    .unwrap();

    let (mut stream, _) = ClientBuilder::from_uri(uri)
        .limits(Limits::unlimited())
        .connector(&Connector::Plain)
        .connect()
        .await?;

    while let Some(msg) = stream.next().await {
        let msg = msg?;

        if msg.is_text() || msg.is_binary() {
            stream.send(msg).await?;
        }
    }

    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Error> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let total = loop {
        let attempt_deadline = deadline.min(Instant::now() + Duration::from_secs(1));
        let error = match timeout_at(attempt_deadline, get_case_count()).await {
            Ok(Ok(total)) => break total,
            Ok(Err(error)) => error,
            Err(error) => io::Error::new(io::ErrorKind::TimedOut, error).into(),
        };
        if Instant::now() >= deadline {
            eprintln!("Autobahn server was not ready within 30 seconds: {error}");
            return Err(error);
        }
        sleep_until(deadline.min(Instant::now() + Duration::from_millis(100))).await;
    };
    println!("Running {total} tests");

    for case in 1..=total {
        if let Err(e) = run_test(case).await {
            match e {
                Error::Protocol(_) => {}
                _ => eprintln!("Testcase failed: {e:?}"),
            }
        };
    }

    update_reports().await?;

    Ok(())
}

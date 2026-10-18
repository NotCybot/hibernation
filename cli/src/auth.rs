use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use centaurus::error::Result;
use centaurus::eyre::ContextCompat;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tokio::{select, spawn};
use tracing::{info, warn};
use url::Url;

use crate::api::ApiClient;
use crate::config::Config;

/// Port the frontend sends the CLI login code to (`http://localhost:16401?code=...`)
const CODE_SERVER_PORT: u16 = 16401;

pub async fn get_tty_url() -> Url {
  loop {
    info!("Please enter the URL of the hibernation server.");
    print!("URL: ");
    std::io::stdout().flush().unwrap();
    let mut lines = BufReader::new(io::stdin()).lines();
    let Some(url_input) = lines.next_line().await.ok().flatten() else {
      warn!("Failed to read URL from input");
      continue;
    };
    let url_input = url_input.trim();
    let Ok(url) = Url::parse(url_input) else {
      warn!("Invalid URL format");
      continue;
    };

    break url;
  }
}

pub async fn get_tty_token(config: &mut Config, config_path: Option<PathBuf>) -> String {
  info!(
    "Opening browser for authentication: {}auth/cli",
    config.app_url
  );
  if opener::open(format!("{}auth/cli", config.app_url)).is_err() {
    warn!("Failed to open browser for authentication.");
  }

  info!("Waiting for authentication...");

  let listener = TcpListener::bind(("0.0.0.0", CODE_SERVER_PORT)).await;
  let mut server = match listener.map(|l| CodeServer::new(l, config.app_url.clone())) {
    Ok(server) => {
      info!("If the authentication fails, please enter the code manually.");
      server
    }
    Err(e) => {
      warn!(
        "Failed to start authentication server. Please enter the code manually. Error: {}",
        e
      );
      CodeServer::dummy()
    }
  };

  let token = loop {
    print!("Code: ");
    std::io::stdout().flush().unwrap();
    let mut lines = BufReader::new(io::stdin()).lines();

    select! {
      Some(token) = server.wait_for_code() => {
        info!("\nGot token from authentication server.");
        break token;
      }
      Ok(Some(code)) = lines.next_line() => {
        let code = code.trim().to_string();
        if let Ok(token) = ApiClient::request_token(config.app_url.clone(), &code).await {
          break token;
        } else {
          warn!("Invalid code. Please try again.");
        }
      }
      else => {
        warn!("Failed to read code from input");
      }
    }
  };

  server.cleanup();
  config.token = Some(token.clone());
  if config.save(config_path).await.is_err() {
    warn!("Failed to save config");
  }
  info!("Authenticated successfully.");

  token
}

pub struct CodeServer {
  receiver: oneshot::Receiver<Option<String>>,
  task: JoinHandle<()>,
}

impl CodeServer {
  pub fn new(listener: TcpListener, app_url: Url) -> Self {
    let (sender, receiver) = oneshot::channel();

    let task = spawn(async {
      sender
        .send(
          code_server(listener, app_url)
            .await
            .map_err(|err| {
              warn!("Error in code server: {:?}", err);
              err
            })
            .ok(),
        )
        .unwrap_or_else(|_| warn!("Failed to send code result"));
    });

    Self { receiver, task }
  }

  pub fn dummy() -> Self {
    let (sender, receiver) = oneshot::channel();
    Self {
      receiver,
      task: spawn(async {
        sleep(Duration::from_hours(100000)).await;
        let _s = sender;
      }),
    }
  }

  pub fn cleanup(&self) {
    self.task.abort();
  }

  pub async fn wait_for_code(&mut self) -> Option<String> {
    (&mut self.receiver).await.ok().flatten()
  }
}

async fn code_server(listener: TcpListener, app_url: Url) -> Result<String> {
  let (mut stream, _) = listener.accept().await?;
  let (reader, mut writer) = stream.split();
  let mut reader = BufReader::new(reader);

  let mut request_line = String::new();
  reader.read_line(&mut request_line).await?;

  let token_result = match parse_code(&request_line) {
    Ok(code) => ApiClient::request_token(app_url, &code).await,
    Err(e) => Err(e),
  };

  let body = if token_result.is_ok() { "OK" } else { "Error" };
  let res_code = if token_result.is_ok() {
    "200 OK"
  } else {
    "400 Bad Request"
  };

  let response = format!(
    "HTTP/1.1 {}\r\n\
Content-Type: text/html; charset=utf-8\r\n\
Content-Length: {}\r\n\
Connection: close\r\n\
Access-Control-Allow-Origin: *\r\n\
\r\n\
{}",
    res_code,
    body.len(),
    body
  );

  writer.write_all(response.as_bytes()).await?;
  writer.flush().await?;

  let _ = stream.shutdown().await;

  sleep(Duration::from_millis(250)).await;

  token_result
}

/// Extract the `code` query parameter from an HTTP request line like `GET /?code=xyz HTTP/1.1`.
/// The method is not checked: a CORS preflight carrying the code is as good as the GET.
fn parse_code(request_line: &str) -> Result<String> {
  let path = request_line
    .split_whitespace()
    .nth(1)
    .context("Invalid request")?;

  let url = Url::parse("http://localhost")?.join(path)?;
  let code = url
    .query_pairs()
    .find(|(k, _)| k == "code")
    .map(|(_, v)| v.into_owned())
    .filter(|c| !c.is_empty())
    .context("Code not found in query parameters")?;

  Ok(code)
}

#[cfg(test)]
mod test {
  use std::time::Duration;

  use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
  };
  use url::Url;
  use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
  };

  use super::{CodeServer, parse_code};

  #[test]
  fn parse_code_cases() {
    for (line, code) in [
      ("GET /?code=abc HTTP/1.1\r\n", "abc"),
      ("GET /?code=abc HTTP/1.1", "abc"),
      ("GET /callback?user=x&code=abc HTTP/1.1\r\n", "abc"),
      ("GET /?code=a%20b%26c HTTP/1.1\r\n", "a b&c"),
      ("GET /?code=first&code=second HTTP/1.1\r\n", "first"),
      // A CORS preflight carries the code too
      ("OPTIONS /?code=abc HTTP/1.1\r\n", "abc"),
    ] {
      assert_eq!(parse_code(line).unwrap(), code, "{line:?}");
    }
    for line in [
      "",
      "GET",
      "GET / HTTP/1.1",
      "GET /?other=abc HTTP/1.1",
      "GET /?code= HTTP/1.1",
      "GET /?code HTTP/1.1",
      "\u{0}\u{1} garbage",
    ] {
      assert!(parse_code(line).is_err(), "{line:?}");
    }
  }

  async fn request(listener_addr: std::net::SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(listener_addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut res = String::new();
    stream.read_to_string(&mut res).await.unwrap();
    res
  }

  async fn server(app: &MockServer) -> (CodeServer, std::net::SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    (
      CodeServer::new(listener, Url::parse(&app.uri()).unwrap()),
      addr,
    )
  }

  #[tokio::test]
  async fn code_server_exchanges_code_for_token() {
    let app = MockServer::start().await;
    Mock::given(method("PUT"))
      .and(path("/api/cli"))
      .and(query_param("code", "abc"))
      .respond_with(ResponseTemplate::new(200).set_body_string("tok"))
      .expect(1)
      .mount(&app)
      .await;

    let (mut server, addr) = server(&app).await;
    let res = request(addr, "GET /?code=abc HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(res.starts_with("HTTP/1.1 200 OK\r\n"), "{res}");
    assert!(res.contains("Access-Control-Allow-Origin: *\r\n"));
    assert!(res.ends_with("\r\n\r\nOK"));
    assert_eq!(server.wait_for_code().await.as_deref(), Some("tok"));
  }

  #[tokio::test]
  async fn code_server_rejects_bad_requests() {
    let app = MockServer::start().await;
    for req in [
      "GET / HTTP/1.1\r\n\r\n",
      "GET /?code=unknown HTTP/1.1\r\n\r\n",
    ] {
      let (mut server, addr) = server(&app).await;
      let res = request(addr, req).await;
      assert!(res.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{res}");
      assert!(res.ends_with("\r\n\r\nError"));
      assert_eq!(server.wait_for_code().await, None);
    }
  }

  #[tokio::test]
  async fn dummy_server_never_yields_until_cleanup() {
    let mut server = CodeServer::dummy();
    assert!(
      timeout(Duration::from_millis(50), server.wait_for_code())
        .await
        .is_err()
    );
    server.cleanup();
    assert_eq!(server.wait_for_code().await, None);
  }
}

//! Synchronous serve-sidecar client (PR 6): reset-then-complete over HTTP.
//!
//! Card Studio has no async runtime, so this is synchronous ureq with the
//! ingestion client's posture ported in: 2 s connect timeout, 20 s
//! request timeout, one backoff retry, `NoCall` (empty calls) falling
//! through to the caller's deterministic path, and vocabulary-validated
//! parsing of the model answer. The engine keeps one process-global
//! conversation, so every turn is `POST /reset` before `POST /complete`
//! with the bare data text as input (verbs leak into grounded fields).

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Reset-then-complete request body.
#[derive(Debug, Serialize)]
struct CompleteRequest<'a> {
    input: &'a str,
}

/// One tool call returned by the engine.
#[derive(Debug, Clone, PartialEq)]
pub struct ServeCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Envelope subset we read from `POST /complete`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct ServeResponse {
    #[serde(default)]
    function_calls: Vec<RawCall>,
    #[serde(default)]
    suppressed_calls: Vec<RawCall>,
    #[serde(default)]
    confidence: Option<f64>,
    #[serde(default)]
    reasoning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct RawCall {
    name: String,
    #[serde(default)]
    arguments: serde_json::Value,
}

/// One answered turn, carrying both the winning call and the confidence.
#[derive(Debug, Clone, PartialEq)]
pub struct ServeTurn {
    pub call: ServeCall,
    /// True when the call came from `suppressed_calls` (the gate withheld
    /// it): usable, but discounted downstream.
    pub withheld: bool,
    pub confidence: Option<f64>,
    pub reasoning: Option<String>,
}

#[derive(Debug)]
pub enum ClientError {
    Http(String),
    Api {
        status_code: u16,
        message: String,
    },
    Json(serde_json::Error),
    /// The engine returned no call (and no withheld call): off-topic for
    /// the toolset. Callers fall through to their deterministic path.
    NoCall,
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Http(error) => write!(f, "HTTP error: {error}"),
            ClientError::Api {
                status_code,
                message,
            } => write!(f, "API error (status {status_code}): {message}"),
            ClientError::Json(error) => write!(f, "bad JSON: {error}"),
            ClientError::NoCall => {
                write!(f, "the engine produced no grounded call for this input")
            }
        }
    }
}

impl std::error::Error for ClientError {}

impl From<serde_json::Error> for ClientError {
    fn from(error: serde_json::Error) -> Self {
        ClientError::Json(error)
    }
}

/// One HTTP endpoint, reset then completed over it. Cloneable: one
/// shared agent (and connection pool) per instance instead of a new
/// TCP connection per POST.
#[derive(Clone)]
pub struct ServeClient {
    base_url: String,
    agent: ureq::Agent,
    max_attempts: usize,
}

impl ServeClient {
    /// Vocabulary-validated turn parsing is the callers' job (see the
    /// Card Doctor call layer); this client delivers raw envelope calls.
    pub fn new(port: u16) -> Self {
        Self::with_base_url(format!("http://127.0.0.1:{port}"), 2)
    }

    /// Test seam: arbitrary base URL plus retry budget. Tests that drive
    /// refusal paths produce them through real non-2xx statuses, so the
    /// behavior under test is ureq's own, not a test double's.
    pub fn with_base_url(base_url: String, max_attempts: usize) -> Self {
        Self {
            base_url,
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(2))
                .timeout(Duration::from_secs(20))
                .max_idle_connections(0)
                .max_idle_connections_per_host(0)
                .build(),
            max_attempts: max_attempts.max(1),
        }
    }

    fn post(&self, path: &str, body: Option<&CompleteRequest>) -> Result<String, ClientError> {
        let request = self
            .agent
            .post(&format!("{}{path}", self.base_url))
            .set("Content-Type", "application/json")
            .set("Connection", "close");
        let response = match body {
            // ureq's json feature is off in this tree, so serialize by hand.
            Some(payload) => request.send_string(&serde_json::to_string(payload)?),
            None => request.call(),
        };
        let response = match response {
            // Status lines reach us as errors in ANY toolchain: a non-2xx
            // from a sidecar is always an Api refusal, never a transport
            // error, and Api refuses never retry.
            Err(ureq::Error::Status(status, refused)) => {
                let message = refused
                    .into_string()
                    .unwrap_or_else(|_| "refused".to_string());
                return Err(ClientError::Api {
                    status_code: status,
                    message,
                });
            }
            Err(other) => return Err(ClientError::Http(other.to_string())),
            Ok(response) => response,
        };
        let status = response.status();
        if !(200..300).contains(&status) {
            let message = response.into_string().unwrap_or_default();
            return Err(ClientError::Api {
                status_code: status,
                message,
            });
        }
        response
            .into_string()
            .map_err(|error| ClientError::Http(error.to_string()))
    }

    /// Raw reset round-trip used by the serve layer as the health check:
    /// transport failures stay `Http`, HTTP error statuses come back as
    /// `Api`. (Applies the same distinction the health contract needs —
    /// a refusing but alive process is healthy, a dead socket is not —
    /// without dragging status plumbing through the public `post`.)
    pub(crate) fn health_reset(&self) -> Result<(), ClientError> {
        match self.agent.post(&format!("{}/reset", self.base_url)).call() {
            Ok(response) => {
                if (200..300).contains(&response.status()) {
                    Ok(())
                } else {
                    Err(ClientError::Api {
                        status_code: response.status(),
                        message: "reset refused".to_string(),
                    })
                }
            }
            Err(ureq::Error::Status(status, _)) => Err(ClientError::Api {
                status_code: status,
                message: "reset refused".to_string(),
            }),
            Err(other) => Err(ClientError::Http(other.to_string())),
        }
    }

    /// Asks one self-contained question: reset, then complete. Retries
    /// once on transport failures (not on API errors or refusals).
    pub fn ask(&self, input: &str) -> Result<ServeTurn, ClientError> {
        let mut last_error: Option<ClientError> = None;
        for attempt in 0..self.max_attempts {
            match self.ask_once(input) {
                Ok(turn) => return Ok(turn),
                Err(ClientError::Http(error)) => {
                    last_error = Some(ClientError::Http(error));
                    if attempt + 1 < self.max_attempts {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                Err(other) => return Err(other),
            }
        }
        Err(last_error.expect("attempt loop always records an error"))
    }

    fn ask_once(&self, input: &str) -> Result<ServeTurn, ClientError> {
        self.post("/reset", None)?;
        let body = self.post("/complete", Some(&CompleteRequest { input }))?;
        let response: ServeResponse = serde_json::from_str(&body)?;
        let (call, withheld) = if let Some(first) = response.function_calls.into_iter().next() {
            (first, false)
        } else if let Some(first) = response.suppressed_calls.into_iter().next() {
            (first, true)
        } else {
            return Err(ClientError::NoCall);
        };
        Ok(ServeTurn {
            call: ServeCall {
                name: call.name,
                arguments: call.arguments,
            },
            withheld,
            confidence: response.confidence,
            reasoning: response.reasoning,
        })
    }
}

//! Outbound email: the one path the platform has.
//!
//! The BFF never talks to a mail server — it relays through `POST /v1/email`
//! with the operator token (`frontend/src/lib/bff/relay.ts`), so the platform
//! has one transport and one template set.
//!
//! Sending is **synchronous, with no outbox and no retry**. The Python design
//! this replaces had a durable `email_outbox` table drained by the dispatcher's
//! leader tick, and that is deliberately not what shipped: the relay already
//! treats failure as data, because every caller has a usable fallback (an invite
//! link to copy, an enumeration-safe 202). An outbox would add a table, a drain
//! loop and a retry policy to make reliable something no flow blocks on. When
//! one does, the outbox is the right answer then.
//!
//! The same reasoning is why [`Backend::Off`] answers 503 rather than pretending
//! to have sent something: a caller that can fall back needs to know it must.
//!
//! **Transport is the SES API, not SES's SMTP interface.** The SMTP interface
//! cannot accept credentials derived from temporary ones, which is the whole
//! reason `infrastructure.md` §9.3 budgeted a second long-lived IAM access key
//! and then scheduled its retirement. Signing the API call with the pod's EKS
//! Pod Identity credentials takes that exit before the credential existed.
//!
//! Bodies are plain text assembled by `format!`. No template engine: there are
//! a handful of them, each a few lines, and the alternative is a dependency
//! plus a directory of files that can drift from the `params` its caller sends.
//! A template's required params are named in one place — [`render`] — and a
//! missing one is a 422 the caller sees immediately, not a blank in an email
//! someone already received.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

/// A message as the BFF relays it. Mirrors `relayEmail`'s body exactly.
#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    /// The recipient address.
    pub to: String,
    /// Which template to render (`invite`, `password_reset`, `signup_verify`,
    /// `run_report`).
    pub template: String,
    /// Template parameters; what is required depends on the template.
    #[serde(default)]
    pub params: BTreeMap<String, Value>,
    /// The tenant this message is about, when it is about one. Carried for the
    /// audit trail, never rendered.
    #[serde(default)]
    pub org: Option<String>,
}

/// Why a send did not happen. Each maps to one status in the route.
#[derive(Debug, thiserror::Error)]
pub enum MailError {
    /// No such template, or a required parameter was missing — the caller's bug,
    /// and fixable only by the caller. 422.
    #[error("{0}")]
    Invalid(String),
    /// Email is not configured in this environment. 503: the relay turns this
    /// into `false` and every caller already has a fallback.
    #[error("email is not configured in this environment")]
    NotConfigured,
    /// SES accepted the request and refused the message, or the call failed.
    /// 502. The cause is reached through `.source()`.
    #[error("sending through SES")]
    Transport {
        #[from]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// A rendered message, ready to hand to a transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// The subject line.
    pub subject: String,
    /// The plain-text body.
    pub body: String,
}

/// Pull a required string parameter, or name the one that is missing.
fn required<'a>(params: &'a BTreeMap<String, Value>, key: &str) -> Result<&'a str, MailError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| MailError::Invalid(format!("template parameter {key:?} is required")))
}

/// Render a template. The single place a template's required parameters are
/// declared, so adding one cannot be forgotten in the caller.
///
/// Unknown templates are an error rather than a generic fallback: a typo in a
/// `template:` string should fail loudly at the relay, not deliver a blank
/// message that looks sent.
pub fn render(message: &Message) -> Result<Rendered, MailError> {
    let params = &message.params;
    match message.template.as_str() {
        "invite" => {
            let org = required(params, "org")?;
            let role = required(params, "role")?;
            let invited_by = required(params, "invited_by")?;
            let link = required(params, "link")?;
            Ok(Rendered {
                subject: format!("You have been invited to {org} on DupeZero"),
                body: format!(
                    "{invited_by} invited you to the {org} workspace on DupeZero as \
                     {role}.\n\nAccept the invitation:\n{link}\n\nThe link expires in seven \
                     days. If you were not expecting this, you can ignore this message.\n"
                ),
            })
        }
        "password_reset" => {
            let link = required(params, "link")?;
            Ok(Rendered {
                subject: "Reset your DupeZero password".to_string(),
                body: format!(
                    "Someone asked to reset the password for this address on \
                     DupeZero.\n\nChoose a new password:\n{link}\n\nThe link expires in one \
                     hour and can be used once. If this was not you, no action is needed — \
                     your password has not changed.\n"
                ),
            })
        }
        "signup_verify" => {
            let workspace = required(params, "workspace")?;
            let link = required(params, "link")?;
            Ok(Rendered {
                subject: format!("Confirm your email to create {workspace}"),
                body: format!(
                    "Confirm this address to finish creating the {workspace} workspace on \
                     DupeZero.\n\nConfirm and open your workspace:\n{link}\n\nThe link \
                     expires in 24 hours. Nothing is created until you confirm, and the \
                     workspace name is not held past that. If you did not request this, you \
                     can ignore this message.\n"
                ),
            })
        }
        "run_report" => {
            let org = required(params, "org")?;
            let kind = required(params, "kind")?;
            let state = required(params, "state")?;
            let job_id = required(params, "job_id")?;
            // Optional, because what a report can say depends on how the job
            // ended: a failure has an error and no counts, a success the
            // reverse, and a job that never reached the engine has neither.
            let optional = |key: &str| params.get(key).and_then(Value::as_str).unwrap_or("");
            let run_id = optional("run_id");
            let error = optional("error");
            let summary = optional("summary");

            // Assembled as sections and joined rather than pushed: clippy's
            // `format_push_string` objects to `push_str(&format!(..))`, and a
            // section list also makes "which parts are conditional" readable.
            let mut sections = vec![format!("The {kind} job on {org} {state}.")];
            let mut facts = format!("job {job_id}");
            if !run_id.is_empty() {
                facts = format!("{facts}\nrun {run_id}");
            }
            sections.push(facts);
            if !summary.is_empty() {
                sections.push(format!("What it changed:\n{summary}"));
            }
            if !error.is_empty() {
                // Already chain-flattened by the dispatcher (`error_detail`),
                // so it reads as a sentence rather than the outermost
                // wrapper's Display.
                sections.push(format!("Why it failed:\n{error}"));
            }
            sections.push(
                "This report was sent because this workspace has run reporting configured. \
                 Ask an operator to remove ER_RUN_REPORT_TO to stop it."
                    .to_string(),
            );
            let body = format!("{}\n", sections.join("\n\n"));
            Ok(Rendered {
                subject: format!("{org}: {kind} {state}"),
                body,
            })
        }
        other => Err(MailError::Invalid(format!("unknown template {other:?}"))),
    }
}

/// How this environment sends mail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    /// Send through the SES API.
    Ses {
        /// The verified `From` address.
        from: String,
        /// An SES configuration set, for bounce and complaint tracking (§7.5).
        configuration_set: Option<String>,
    },
    /// Render and log instead of sending. The local and per-developer path:
    /// SES's sandbox only delivers to verified recipients, and a laptop stack
    /// has no AWS at all, so the link a flow depends on arrives in the server
    /// log. **Logs the body, which contains a single-use credential** — only
    /// ever for dev.
    Log,
    /// Email is not configured. Every send is [`MailError::NotConfigured`],
    /// which is the behaviour this repo had before the route existed.
    Off,
}

/// Decide the backend from a key lookup.
///
/// Takes a lookup rather than reading the environment directly so it is
/// testable: Rust 2024 makes `std::env::set_var` `unsafe`, and this workspace
/// denies `unsafe_code` — so a test that mutated the process environment could
/// not compile, and one that read it would depend on the runner's.
///
/// A misconfiguration degrades to [`Backend::Off`] with a warning rather than
/// failing the boot. Mail is best-effort by design (every caller has a
/// fallback), so refusing to start would turn a cosmetic misconfiguration into
/// an outage of the whole control plane.
fn backend_from(get: &dyn Fn(&str) -> Option<String>) -> Backend {
    let selected = get("ER_MAIL_BACKEND").unwrap_or_default();
    match selected.trim() {
        "log" => Backend::Log,
        "ses" => {
            let from = get("ER_MAIL_FROM")
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            let Some(from) = from else {
                eprintln!(
                    "er-server: ER_MAIL_BACKEND=ses but ER_MAIL_FROM is unset or empty — \
                     email is disabled"
                );
                return Backend::Off;
            };
            Backend::Ses {
                from,
                configuration_set: get("ER_MAIL_CONFIGURATION_SET")
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty()),
            }
        }
        other => {
            if !other.is_empty() && other != "off" {
                eprintln!(
                    "er-server: unrecognised ER_MAIL_BACKEND {other:?} \
                     (expected ses, log or off) — email is disabled"
                );
            }
            Backend::Off
        }
    }
}

/// The mailer: a backend plus, for SES, its client.
pub struct Mailer {
    backend: Backend,
    ses: Option<aws_sdk_sesv2::Client>,
}

impl std::fmt::Debug for Mailer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mailer")
            .field("backend", &self.backend)
            .finish_non_exhaustive()
    }
}

impl Mailer {
    /// A mailer that refuses every send.
    #[must_use]
    pub fn off() -> Self {
        Self {
            backend: Backend::Off,
            ses: None,
        }
    }

    /// A mailer that renders to the log.
    #[must_use]
    pub fn logging() -> Self {
        Self {
            backend: Backend::Log,
            ses: None,
        }
    }

    /// Build from the environment.
    ///
    /// `ER_MAIL_BACKEND` selects `ses`, `log` or `off` (the default, and the
    /// value for any unrecognised setting — an environment that meant to send
    /// mail and typo'd the backend gets 503s and a warning, not a crash at
    /// boot). SES additionally needs `ER_MAIL_FROM`, and takes an optional
    /// `ER_MAIL_CONFIGURATION_SET`.
    ///
    /// Credentials come from the default provider chain, which in-cluster means
    /// the container-credentials provider that EKS Pod Identity populates. There
    /// is nothing to configure and no key to rotate.
    pub async fn from_env() -> Self {
        Self::with_backend(backend_from(&|key| std::env::var(key).ok())).await
    }

    /// Build a mailer for an already-decided backend, constructing the SES
    /// client only when one is needed.
    pub async fn with_backend(backend: Backend) -> Self {
        if !matches!(backend, Backend::Ses { .. }) {
            return Self { backend, ses: None };
        }
        // An explicit ring-backed HTTPS client: the SDK's default resolves TLS
        // to aws-lc-rs, whose -sys crate needs cmake in the build image (see
        // this crate's Cargo.toml).
        let http = aws_smithy_http_client::Builder::new()
            .tls_provider(aws_smithy_http_client::tls::Provider::Rustls(
                aws_smithy_http_client::tls::rustls_provider::CryptoMode::Ring,
            ))
            .build_https();
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .http_client(http)
            .load()
            .await;
        Self {
            backend,
            ses: Some(aws_sdk_sesv2::Client::new(&config)),
        }
    }

    /// Which backend this mailer uses.
    #[must_use]
    pub fn backend(&self) -> &Backend {
        &self.backend
    }

    /// Render and send. Rendering happens for every backend, including
    /// [`Backend::Off`] — a bad template is the caller's bug and should be
    /// reported as one in every environment, not hidden by whichever one has
    /// mail turned off.
    pub async fn send(&self, message: &Message) -> Result<(), MailError> {
        let rendered = render(message)?;
        match (&self.backend, &self.ses) {
            // `Ses` without a client is only constructible by hand —
            // `with_backend` always builds one for that variant — so it is
            // treated as unconfigured rather than panicking.
            (Backend::Off, _) | (Backend::Ses { .. }, None) => Err(MailError::NotConfigured),
            (Backend::Log, _) => {
                eprintln!(
                    "er-server: [mail:log] to={} template={} org={}\n  subject: {}\n{}",
                    message.to,
                    message.template,
                    message.org.as_deref().unwrap_or("-"),
                    rendered.subject,
                    rendered.body
                );
                Ok(())
            }
            (
                Backend::Ses {
                    from,
                    configuration_set,
                },
                Some(client),
            ) => {
                let body = aws_sdk_sesv2::types::Body::builder()
                    .text(
                        aws_sdk_sesv2::types::Content::builder()
                            .data(&rendered.body)
                            .charset("UTF-8")
                            .build()
                            .map_err(boxed)?,
                    )
                    .build();
                let simple = aws_sdk_sesv2::types::Message::builder()
                    .subject(
                        aws_sdk_sesv2::types::Content::builder()
                            .data(&rendered.subject)
                            .charset("UTF-8")
                            .build()
                            .map_err(boxed)?,
                    )
                    .body(body)
                    .build();
                let content = aws_sdk_sesv2::types::EmailContent::builder()
                    .simple(simple)
                    .build();
                let destination = aws_sdk_sesv2::types::Destination::builder()
                    .to_addresses(&message.to)
                    .build();
                let mut call = client
                    .send_email()
                    .from_email_address(from)
                    .destination(destination)
                    .content(content);
                if let Some(set) = configuration_set {
                    call = call.configuration_set_name(set);
                }
                call.send().await.map_err(boxed)?;
                Ok(())
            }
        }
    }
}

fn boxed<E>(error: E) -> MailError
where
    E: std::error::Error + Send + Sync + 'static,
{
    MailError::Transport {
        source: Box::new(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(template: &str, params: &[(&str, &str)]) -> Message {
        Message {
            to: "person@example.com".to_string(),
            template: template.to_string(),
            params: params
                .iter()
                .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
                .collect(),
            org: None,
        }
    }

    fn invite_params() -> Vec<(&'static str, &'static str)> {
        vec![
            ("org", "acme"),
            ("role", "steward"),
            ("invited_by", "admin@acme.com"),
            ("link", "https://app.example.com/invite/tok"),
        ]
    }

    #[test]
    fn every_template_the_bff_sends_renders() {
        // These three strings are the contract with relay.ts's callers. A
        // rename on either side must break this test.
        let invite = render(&message("invite", &invite_params())).expect("invite renders");
        assert!(invite.subject.contains("acme"));
        assert!(invite.body.contains("https://app.example.com/invite/tok"));
        assert!(invite.body.contains("steward"));
        assert!(invite.body.contains("admin@acme.com"));

        let reset = render(&message(
            "password_reset",
            &[("link", "https://app.example.com/reset/tok")],
        ))
        .expect("password_reset renders");
        assert!(reset.body.contains("https://app.example.com/reset/tok"));

        let signup = render(&message(
            "signup_verify",
            &[
                ("workspace", "acme"),
                ("link", "https://app.example.com/signup/verify/tok"),
            ],
        ))
        .expect("signup_verify renders");
        assert!(signup.subject.contains("acme"));
        assert!(
            signup
                .body
                .contains("https://app.example.com/signup/verify/tok")
        );
    }

    #[test]
    fn an_unknown_template_is_invalid_not_a_blank_message() {
        let error = render(&message("welcome_aboard", &[])).expect_err("must refuse");
        assert!(matches!(error, MailError::Invalid(_)));
        assert!(error.to_string().contains("welcome_aboard"));
    }

    #[test]
    fn a_missing_parameter_names_itself() {
        for omitted in ["org", "role", "invited_by", "link"] {
            let params: Vec<_> = invite_params()
                .into_iter()
                .filter(|(k, _)| *k != omitted)
                .collect();
            let error = render(&message("invite", &params)).unwrap_err().to_string();
            assert!(
                error.contains(omitted),
                "omitting {omitted:?} should name it, got: {error}"
            );
        }
    }

    #[test]
    fn a_blank_parameter_counts_as_missing() {
        // An empty `link` would render a message whose only purpose is the link.
        let params = vec![
            ("org", "acme"),
            ("role", "steward"),
            ("invited_by", "admin@acme.com"),
            ("link", "   "),
        ];
        assert!(render(&message("invite", &params)).is_err());
    }

    #[tokio::test]
    async fn the_off_backend_refuses_but_still_validates_the_template() {
        let mailer = Mailer::off();
        // A valid message is refused as unconfigured...
        let error = mailer
            .send(&message("invite", &invite_params()))
            .await
            .unwrap_err();
        assert!(matches!(error, MailError::NotConfigured));
        // ...but a bad template is still the caller's bug, in every environment.
        let error = mailer.send(&message("nope", &[])).await.unwrap_err();
        assert!(matches!(error, MailError::Invalid(_)));
    }

    #[tokio::test]
    async fn the_log_backend_accepts_a_renderable_message() {
        let mailer = Mailer::logging();
        assert!(
            mailer
                .send(&message("invite", &invite_params()))
                .await
                .is_ok()
        );
    }

    /// A lookup over a fixed table, standing in for the environment.
    fn envs(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |key| owned.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    #[test]
    fn an_unconfigured_or_misconfigured_environment_resolves_to_off() {
        // Nothing set at all — the default, and today's behaviour.
        assert_eq!(backend_from(&envs(&[])), Backend::Off);
        assert_eq!(
            backend_from(&envs(&[("ER_MAIL_BACKEND", "off")])),
            Backend::Off
        );
        // A typo'd backend degrades rather than failing the boot: mail is
        // best-effort and the control plane has other jobs.
        assert_eq!(
            backend_from(&envs(&[("ER_MAIL_BACKEND", "sendgrid")])),
            Backend::Off
        );
        // ses without a From address cannot send, so it must not look enabled.
        assert_eq!(
            backend_from(&envs(&[("ER_MAIL_BACKEND", "ses")])),
            Backend::Off
        );
        assert_eq!(
            backend_from(&envs(&[("ER_MAIL_BACKEND", "ses"), ("ER_MAIL_FROM", "  ")])),
            Backend::Off
        );
    }

    #[test]
    fn a_configured_environment_resolves_to_its_backend() {
        assert_eq!(
            backend_from(&envs(&[("ER_MAIL_BACKEND", "log")])),
            Backend::Log
        );
        assert_eq!(
            backend_from(&envs(&[
                ("ER_MAIL_BACKEND", "ses"),
                ("ER_MAIL_FROM", " no-reply@example.com "),
            ])),
            Backend::Ses {
                // Trimmed: a trailing space in a Secrets Manager value would
                // otherwise reach SES as part of the address.
                from: "no-reply@example.com".to_string(),
                configuration_set: None,
            }
        );
        assert_eq!(
            backend_from(&envs(&[
                ("ER_MAIL_BACKEND", "ses"),
                ("ER_MAIL_FROM", "no-reply@example.com"),
                ("ER_MAIL_CONFIGURATION_SET", "er-dev"),
            ])),
            Backend::Ses {
                from: "no-reply@example.com".to_string(),
                configuration_set: Some("er-dev".to_string()),
            }
        );
    }
}

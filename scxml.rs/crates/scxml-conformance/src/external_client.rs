//! Harness-as-external-client oracles for W3C manual IRP cases.
//!
//! Some official IRP cases are not SCXML documents but instructions for a
//! human tester acting as an external HTTP client. For each such case this
//! module holds one auditable oracle derived directly from the W3C
//! instructions and bound to the exact SHA-256 of the prepared instruction
//! file, so a changed instruction fails closed instead of silently keeping
//! an outdated oracle.
//!
//! An oracle names three things, all taken from the W3C text:
//!
//! 1. a receiving SCXML session: a harness-authored document (it is **not**
//!    part of the W3C corpus) that follows the IRP `conf:pass`/`conf:fail`
//!    convention, so its only observable result is one `Outcome` log;
//! 2. the HTTP request the W3C tester sends to that session's `BasicHTTP`
//!    `location`, which the harness sends through the same in-process
//!    `http-transport` loopback and the shipped `BasicHTTP` processor that
//!    sessions use;
//! 3. the response the W3C tester must observe.
//!
//! The case passes only when the response matches **and** the session
//! passes the official `Outcome` oracle exactly as an outcome-log case does:
//! one `Outcome` log of `pass` in a session that finished in exactly the
//! `pass` final state ([`crate::oracle::evaluate_pass_termination`]). That
//! proves the message was decoded, added to the session's external queue,
//! and processed.

use scxml_ioprocessor_basichttp::{FORM_MEDIA_TYPE, HttpHeader, HttpRequest, HttpResponse};

use crate::executor::ExecutionResult;
use crate::manifest::LoadedCase;
use crate::oracle::{self, OracleFailure};

/// One W3C manual case whose oracle is the harness acting as the external
/// HTTP client described by the W3C instructions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExternalClientOracle {
    /// Official assertion and test identifier.
    pub case_id: &'static str,
    /// Manifest `start` URI of the W3C instruction file.
    pub start_uri: &'static str,
    /// SHA-256 of the exact W3C instruction bytes the oracle was derived from.
    pub instruction_sha256: &'static str,
    /// Harness-authored receiving document, with `{datamodel}` standing for
    /// the suite data model.
    pub receiver_template: &'static str,
    /// Request method the W3C tester uses.
    pub method: &'static str,
    /// Request headers the W3C tester sends.
    pub headers: &'static [(&'static str, &'static str)],
    /// Request body the W3C tester sends.
    pub body: &'static str,
    /// Exact response status the W3C tester must observe.
    pub expected_status: u16,
}

/// Official case 513 (SCXML C.2, `#BasicHTTPEventProcessor`), from
/// `513/test513.txt`: "send a well formed event to the 'location' URL
/// specified for your SCXML interpreter and check that you get a 200
/// response code back", using `wget --post-data='key1=value1&key2=value2'
/// --header '_scxmleventname: test'` (wget posts
/// `application/x-www-form-urlencoded`). The receiving session passes only
/// when it processes the external event `test`.
pub const CASE_513: ExternalClientOracle = ExternalClientOracle {
    case_id: "513",
    start_uri: "513/test513.txt",
    instruction_sha256: "418df239d9c85bc1fe04be64d1cf23b605a236975f14ed66a1c33fd40f2e059b",
    receiver_template: r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- Harness-authored receiver for W3C manual case 513; not W3C corpus bytes. -->
<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="{datamodel}" initial="s0">
  <state id="s0">
    <transition event="test" target="pass"/>
    <transition event="*" target="fail"/>
  </state>
  <final id="pass"><onentry><log label="Outcome" expr="'pass'"/></onentry></final>
  <final id="fail"><onentry><log label="Outcome" expr="'fail'"/></onentry></final>
</scxml>
"#,
    method: "POST",
    headers: &[
        ("Content-Type", FORM_MEDIA_TYPE),
        ("_scxmleventname", "test"),
    ],
    body: "key1=value1&key2=value2",
    expected_status: 200,
};

/// Every harness-as-external-client oracle, by official case.
pub const ORACLES: &[ExternalClientOracle] = &[CASE_513];

/// Why a case that names an external-client oracle cannot use it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OracleDrift {
    /// Stable reason code.
    pub code: &'static str,
    /// Bounded explanation without source payloads.
    pub message: String,
}

/// Selects the external-client oracle for `case`: `None` when the case has
/// none, `Err` when its instruction identity differs from the one the
/// oracle was derived from.
///
/// # Errors
///
/// [`OracleDrift`] for a changed start URI or instruction digest.
#[must_use]
pub fn select(case: &LoadedCase) -> Option<Result<ExternalClientOracle, OracleDrift>> {
    ORACLES
        .iter()
        .find(|oracle| oracle.case_id == case.manifest.id)
        .map(|oracle| {
            if case.manifest.start_uri != oracle.start_uri
                || case.manifest.source_digest.as_deref() != Some(oracle.instruction_sha256)
            {
                Err(OracleDrift {
                    code: "external-client-oracle-drift",
                    message: "the W3C instruction identity differs from the one the external-client oracle was derived from".into(),
                })
            } else {
                Ok(*oracle)
            }
        })
}

impl ExternalClientOracle {
    /// The receiving document for `datamodel`.
    #[must_use]
    pub fn receiver(&self, datamodel: &str) -> String {
        self.receiver_template.replace("{datamodel}", datamodel)
    }

    /// The W3C tester's request, addressed to `location`.
    #[must_use]
    pub fn request(&self, location: &str) -> HttpRequest {
        HttpRequest {
            method: self.method.into(),
            uri: location.into(),
            headers: self
                .headers
                .iter()
                .map(|(name, value)| HttpHeader {
                    name: (*name).into(),
                    value: (*value).into(),
                })
                .collect(),
            body: self.body.as_bytes().to_vec(),
        }
    }

    /// Decides the case: the response the W3C tester must observe, then the
    /// receiving session's official `Outcome` oracle and its termination in
    /// exactly the `pass` final state.
    ///
    /// # Errors
    ///
    /// [`OracleFailure`] from the first failing check, in that order.
    pub fn evaluate(
        &self,
        response: &HttpResponse,
        execution: &ExecutionResult,
    ) -> Result<(), OracleFailure> {
        self.evaluate_response(response)?;
        oracle::evaluate_outcome_logs(&execution.logs)?;
        oracle::evaluate_pass_termination(execution)
    }

    /// Checks the response the W3C tester must observe.
    ///
    /// # Errors
    ///
    /// [`OracleFailure`] when the status differs.
    pub fn evaluate_response(&self, response: &HttpResponse) -> Result<(), OracleFailure> {
        if response.status == self.expected_status {
            Ok(())
        } else {
            Err(OracleFailure {
                code: "external-client-status-mismatch",
                message: format!(
                    "the external client expected HTTP {} but received {}",
                    self.expected_status, response.status
                ),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_513_oracle_is_bound_to_the_pinned_w3c_instruction_bytes() {
        let instruction =
            include_bytes!("../../../tests/conformance/w3c-irp/upstream/sources/513/test513.txt");
        assert_eq!(
            crate::manifest::sha256_bytes(instruction),
            CASE_513.instruction_sha256
        );
        let text = String::from_utf8_lossy(instruction);
        // The request and expectation are the W3C tester's own.
        assert!(text.contains("--post-data='key1=value1&key2=value2'"));
        assert!(text.contains("--header '_scxmleventname: test'"));
        assert!(text.contains("200 response code"));
        assert_eq!(CASE_513.body, "key1=value1&key2=value2");
        assert_eq!(CASE_513.expected_status, 200);
    }

    #[test]
    fn response_oracle_requires_the_exact_status() {
        let response = |status| HttpResponse {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        };
        assert!(CASE_513.evaluate_response(&response(200)).is_ok());
        assert_eq!(
            CASE_513
                .evaluate_response(&response(204))
                .map_err(|failure| failure.code),
            Err("external-client-status-mismatch")
        );
    }
}

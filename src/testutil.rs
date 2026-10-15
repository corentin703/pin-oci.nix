use std::cell::RefCell;
use std::path::PathBuf;

use crate::nix::{Evaluator, NixError};

pub(crate) fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pin-oci-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub(crate) struct StubEvaluator {
    response: Result<String, String>,
    /// `(installable, apply)` of every call.
    pub(crate) calls: RefCell<Vec<(String, String)>>,
}

impl StubEvaluator {
    pub(crate) fn ok(json: &str) -> Self {
        Self {
            response: Ok(json.to_owned()),
            calls: RefCell::default(),
        }
    }

    pub(crate) fn failing(stderr: &str) -> Self {
        Self {
            response: Err(stderr.to_owned()),
            calls: RefCell::default(),
        }
    }
}

impl Evaluator for StubEvaluator {
    fn eval_json(&self, installable: &str, apply: &str) -> Result<String, NixError> {
        self.calls
            .borrow_mut()
            .push((installable.to_owned(), apply.to_owned()));
        match &self.response {
            Ok(json) => Ok(json.clone()),
            Err(stderr) => Err(NixError::Failed {
                code: Some(1),
                stderr: stderr.clone(),
            }),
        }
    }
}

use std::cell::RefCell;
use std::path::PathBuf;

use std::collections::HashMap;

use crate::nix::{Evaluator, NixError};
use crate::registry::{RegistryError, Runtime};

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
    /// When not empty, the response is looked up by exact installable instead.
    routes: Vec<(String, Result<String, String>)>,
}

impl StubEvaluator {
    pub(crate) fn ok(json: &str) -> Self {
        Self {
            response: Ok(json.to_owned()),
            calls: RefCell::default(),
            routes: Vec::new(),
        }
    }

    /// An evaluator answering per installable; an unlisted one is a test bug.
    pub(crate) fn routed() -> Self {
        Self {
            response: Err("no route".to_owned()),
            calls: RefCell::default(),
            routes: Vec::new(),
        }
    }

    pub(crate) fn route(mut self, installable: &str, json: &str) -> Self {
        self.routes
            .push((installable.to_owned(), Ok(json.to_owned())));
        self
    }

    pub(crate) fn route_failing(mut self, installable: &str, stderr: &str) -> Self {
        self.routes
            .push((installable.to_owned(), Err(stderr.to_owned())));
        self
    }

    pub(crate) fn failing(stderr: &str) -> Self {
        Self {
            response: Err(stderr.to_owned()),
            calls: RefCell::default(),
            routes: Vec::new(),
        }
    }
}

impl Evaluator for StubEvaluator {
    fn eval_json(&self, installable: &str, apply: &str) -> Result<String, NixError> {
        self.calls
            .borrow_mut()
            .push((installable.to_owned(), apply.to_owned()));
        let response = if self.routes.is_empty() {
            &self.response
        } else {
            match self.routes.iter().find(|(i, _)| i == installable) {
                Some((_, response)) => response,
                None => panic!("unexpected nix eval: {installable}"),
            }
        };
        match response {
            Ok(json) => Ok(json.clone()),
            Err(stderr) => Err(NixError::Failed {
                code: Some(1),
                stderr: stderr.clone(),
            }),
        }
    }
}

/// Skopeo stub keyed by the exact argument list.
#[derive(Default)]
pub(crate) struct StubRuntime {
    responses: HashMap<String, Result<String, String>>,
    /// Joined arguments of every call.
    pub(crate) calls: RefCell<Vec<String>>,
}

impl StubRuntime {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn on(mut self, args: &[&str], stdout: &str) -> Self {
        self.responses.insert(args.join(" "), Ok(stdout.to_owned()));
        self
    }

    pub(crate) fn failing(mut self, args: &[&str], stderr: &str) -> Self {
        self.responses
            .insert(args.join(" "), Err(stderr.to_owned()));
        self
    }
}

impl Runtime for StubRuntime {
    fn skopeo(&self, args: &[&str]) -> Result<String, RegistryError> {
        let key = args.join(" ");
        self.calls.borrow_mut().push(key.clone());
        match self.responses.get(&key) {
            Some(Ok(stdout)) => Ok(stdout.clone()),
            Some(Err(stderr)) => Err(RegistryError::Failed {
                code: Some(2),
                stderr: stderr.clone(),
            }),
            None => panic!("unexpected skopeo call: {key}"),
        }
    }
}

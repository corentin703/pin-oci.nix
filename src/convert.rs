//! `pin-oci convert`: a docker-compose file to a pin-oci project module.
//!
//! A useful subset, not compose. Everything this module does not map is reported as a warning
//! (never dropped silently); values it cannot map faithfully are errors. Pure: YAML text in,
//! Nix text and warnings out. No file, no network, no `nix`.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;

use crate::render::{self, Network, Project, Service, Volume};

/// A YAML value. Merge keys (`<<: *anchor`) are applied by the parser.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Yaml {
    Null(()),
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Seq(Vec<Yaml>),
    Map(BTreeMap<String, Yaml>),
}

#[derive(Debug)]
pub(crate) struct Converted {
    pub(crate) nix: String,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConvertError {
    Yaml(String),
    Shape {
        path: String,
        expected: &'static str,
    },
    Name {
        kind: &'static str,
        name: String,
    },
    NoName,
    NoImage {
        service: String,
        build: bool,
    },
    Command {
        path: String,
        reason: &'static str,
    },
    /// Values that cannot be written into a static module as they are.
    Values {
        /// Paths using `${VAR}` / `$VAR`, which nothing can resolve here.
        interpolated: Vec<String>,
        /// Paths of unquoted numbers that YAML would not hand back unchanged.
        numbers: Vec<String>,
    },
}

impl fmt::Display for ConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Yaml(e) => write!(f, "invalid YAML: {e}"),
            Self::Shape { path, expected } => write!(f, "{path}: expected {expected}"),
            Self::Name { kind, name } => write!(
                f,
                "{kind} name '{name}' is not allowed: it must match [a-zA-Z0-9][a-zA-Z0-9_.-]*"
            ),
            Self::NoName => write!(
                f,
                "cannot tell the project name: set `name:` in the file or pass --name"
            ),
            Self::NoImage { service, build } => {
                write!(f, "services.{service}: no `image`")?;
                if *build {
                    write!(f, " (`build` is not supported: pin-oci only runs images)")?;
                }
                Ok(())
            }
            Self::Command { path, reason } => write!(f, "{path}: {reason}"),
            Self::Values {
                interpolated,
                numbers,
            } => {
                let mut sections = Vec::new();
                if !interpolated.is_empty() {
                    sections.push(format!(
                        "variable interpolation (${{VAR}}, $VAR) cannot be resolved into a static module; \
                         write the values in the file (use $$ for a literal $):{}",
                        paths(interpolated)
                    ));
                }
                if !numbers.is_empty() {
                    sections.push(format!(
                        "unquoted numbers are not kept as written (1.10 is read as 1.1, 007 as 7); \
                         quote them:{}",
                        paths(numbers)
                    ));
                }
                write!(f, "{}", sections.join("\n"))
            }
        }
    }
}

fn paths(paths: &[String]) -> String {
    paths.iter().map(|p| format!("\n  - {p}")).collect()
}

impl std::error::Error for ConvertError {}

/// The Docker/Podman name rule the module enforces (BIBLE §5.2).
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// `$VAR`, `${VAR}`; `$$` is an escaped dollar.
fn has_interpolation(s: &str) -> bool {
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            continue;
        }
        match chars.peek() {
            Some('$') => {
                chars.next();
            }
            Some(&n) if n == '{' || n == '_' || n.is_ascii_alphabetic() => return true,
            _ => {}
        }
    }
    false
}

/// Splits a compose `command` / `entrypoint` string like a POSIX shell would.
fn shell_words(s: &str) -> Result<Vec<String>, &'static str> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => return Err("unterminated single quote"),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\' | '$' | '`')) => word.push(c),
                            Some(c) => {
                                word.push('\\');
                                word.push(c);
                            }
                            None => return Err("unterminated double quote"),
                        },
                        Some(c) => word.push(c),
                        None => return Err("unterminated double quote"),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some(c) => word.push(c),
                    None => return Err("trailing backslash"),
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

/// The named volume a `volumes` entry refers to (`name:/path[:opts]`), if it is not a bind mount.
fn named_volume(entry: &str) -> Option<&str> {
    let (source, _) = entry.split_once(':')?;
    let bind = source.is_empty() || source.starts_with(['/', '.', '~']);
    (!bind).then_some(source)
}

#[derive(Default)]
struct Converter {
    warnings: Vec<String>,
    interpolated: Vec<String>,
    numbers: Vec<String>,
}

type Map = BTreeMap<String, Yaml>;

impl Converter {
    fn warn(&mut self, path: &str, what: &str) {
        self.warnings.push(format!("{path}: {what}"));
    }

    fn unsupported(&mut self, path: &str) {
        self.warn(path, "not supported, ignored");
    }

    /// A string value, with `$$` unescaped and any interpolation recorded.
    fn text(&mut self, path: &str, s: &str) -> String {
        if has_interpolation(s) {
            self.interpolated.push(path.to_owned());
        }
        s.replace("$$", "$")
    }

    fn scalar(&mut self, path: &str, v: &Yaml) -> Option<String> {
        match v {
            Yaml::Str(s) => Some(self.text(path, s)),
            Yaml::Int(i) => Some(i.to_string()),
            Yaml::Bool(b) => Some(b.to_string()),
            // The parser hands floats back normalized: 1.10 is 1.1 and 007 is 7.0.
            Yaml::Float(f) => {
                self.numbers.push(path.to_owned());
                Some(f.to_string())
            }
            _ => None,
        }
    }

    fn require_scalar(&mut self, path: &str, v: &Yaml) -> Result<String, ConvertError> {
        self.scalar(path, v).ok_or_else(|| ConvertError::Shape {
            path: path.to_owned(),
            expected: "a scalar",
        })
    }

    fn map<'a>(&self, path: &str, v: &'a Yaml) -> Result<&'a Map, ConvertError> {
        match v {
            Yaml::Map(m) => Ok(m),
            _ => Err(ConvertError::Shape {
                path: path.to_owned(),
                expected: "a mapping",
            }),
        }
    }

    /// `null` or a mapping (compose lets `volumes: { data: }` stand for an empty entry).
    fn map_or_empty<'a>(&self, path: &str, v: &'a Yaml) -> Result<Option<&'a Map>, ConvertError> {
        match v {
            Yaml::Null(()) => Ok(None),
            other => self.map(path, other).map(Some),
        }
    }

    fn seq<'a>(&self, path: &str, v: &'a Yaml) -> Result<&'a [Yaml], ConvertError> {
        match v {
            Yaml::Seq(items) => Ok(items),
            _ => Err(ConvertError::Shape {
                path: path.to_owned(),
                expected: "a list",
            }),
        }
    }

    fn strings(&mut self, path: &str, v: &Yaml) -> Result<Vec<String>, ConvertError> {
        let mut out = Vec::new();
        for (i, item) in self.seq(path, v)?.iter().enumerate() {
            out.push(self.require_scalar(&format!("{path}[{i}]"), item)?);
        }
        Ok(out)
    }

    /// `command` / `entrypoint`: a list, or a string split like a shell would.
    fn command(&mut self, path: &str, v: &Yaml) -> Result<Vec<String>, ConvertError> {
        match v {
            Yaml::Str(s) => {
                let text = self.text(path, s);
                shell_words(&text).map_err(|reason| ConvertError::Command {
                    path: path.to_owned(),
                    reason,
                })
            }
            other => self.strings(path, other),
        }
    }

    /// `environment` / `labels`: a mapping or a list of `KEY=value`.
    fn key_values(
        &mut self,
        path: &str,
        v: &Yaml,
        no_value: &str,
    ) -> Result<BTreeMap<String, String>, ConvertError> {
        let mut out = BTreeMap::new();
        match v {
            Yaml::Map(entries) => {
                for (key, value) in entries {
                    let at = format!("{path}.{key}");
                    match value {
                        Yaml::Null(()) => self.warn(&at, no_value),
                        other => {
                            let value = self.require_scalar(&at, other)?;
                            out.insert(key.clone(), value);
                        }
                    }
                }
            }
            Yaml::Seq(items) => {
                for (i, item) in items.iter().enumerate() {
                    let at = format!("{path}[{i}]");
                    let entry = self.require_scalar(&at, item)?;
                    match entry.split_once('=') {
                        Some((key, value)) => {
                            out.insert(key.to_owned(), value.to_owned());
                        }
                        None => self.warn(&format!("{path}.{entry}"), no_value),
                    }
                }
            }
            _ => {
                return Err(ConvertError::Shape {
                    path: path.to_owned(),
                    expected: "a mapping or a list",
                })
            }
        }
        Ok(out)
    }

    fn ports(&mut self, path: &str, v: &Yaml) -> Result<Vec<String>, ConvertError> {
        let mut out = Vec::new();
        for (i, item) in self.seq(path, v)?.iter().enumerate() {
            let at = format!("{path}[{i}]");
            match item {
                Yaml::Map(long) => out.push(self.long_port(&at, long)?),
                other => out.push(self.require_scalar(&at, other)?),
            }
        }
        Ok(out)
    }

    /// `{ target, published, protocol, host_ip }` to `[host_ip:][published:]target[/protocol]`.
    fn long_port(&mut self, path: &str, long: &Map) -> Result<String, ConvertError> {
        let field = |c: &mut Self, key: &str| -> Result<Option<String>, ConvertError> {
            long.get(key)
                .map(|v| c.require_scalar(&format!("{path}.{key}"), v))
                .transpose()
        };
        let target = field(self, "target")?.ok_or_else(|| ConvertError::Shape {
            path: format!("{path}.target"),
            expected: "a port",
        })?;
        let published = field(self, "published")?;
        let host_ip = field(self, "host_ip")?;
        let protocol = field(self, "protocol")?;
        for key in long.keys() {
            if !matches!(
                key.as_str(),
                "target" | "published" | "host_ip" | "protocol"
            ) {
                self.unsupported(&format!("{path}.{key}"));
            }
        }
        let mut out = String::new();
        if let Some(ip) = host_ip {
            out.push_str(&ip);
            out.push(':');
        }
        if let Some(published) = published {
            out.push_str(&published);
            out.push(':');
        }
        out.push_str(&target);
        // `tcp` is the default; keep the suffix only when it says something.
        if let Some(protocol) = protocol.filter(|p| p != "tcp") {
            out.push('/');
            out.push_str(&protocol);
        }
        Ok(out)
    }

    fn volumes(&mut self, path: &str, v: &Yaml) -> Result<Vec<String>, ConvertError> {
        let mut out = Vec::new();
        for (i, item) in self.seq(path, v)?.iter().enumerate() {
            let at = format!("{path}[{i}]");
            let entry = match item {
                Yaml::Map(long) => self.long_volume(&at, long)?,
                other => Some(self.require_scalar(&at, other)?),
            };
            let Some(entry) = entry else { continue };
            if entry
                .split_once(':')
                .is_some_and(|(source, _)| source.starts_with(['.', '~']))
            {
                self.warn(
                    &at,
                    "relative bind mount: the backend resolves it at run time, not relative to the compose file",
                );
            }
            out.push(entry);
        }
        Ok(out)
    }

    /// `{ type, source, target, read_only }` to `source:target[:ro]`.
    fn long_volume(&mut self, path: &str, long: &Map) -> Result<Option<String>, ConvertError> {
        let field = |c: &mut Self, key: &str| -> Result<Option<String>, ConvertError> {
            long.get(key)
                .map(|v| c.require_scalar(&format!("{path}.{key}"), v))
                .transpose()
        };
        let kind = field(self, "type")?;
        let source = field(self, "source")?;
        let target = field(self, "target")?.ok_or_else(|| ConvertError::Shape {
            path: format!("{path}.target"),
            expected: "a path",
        })?;
        let read_only = field(self, "read_only")?.is_some_and(|v| v == "true");
        for key in long.keys() {
            if !matches!(key.as_str(), "type" | "source" | "target" | "read_only") {
                self.unsupported(&format!("{path}.{key}"));
            }
        }
        if kind.as_deref() == Some("tmpfs") {
            self.warn(path, "tmpfs mounts are not supported, ignored");
            return Ok(None);
        }
        let mut out = match source {
            Some(source) => format!("{source}:{target}"),
            None => target,
        };
        if read_only {
            out.push_str(":ro");
        }
        Ok(Some(out))
    }

    fn depends_on(&mut self, path: &str, v: &Yaml) -> Result<Vec<String>, ConvertError> {
        match v {
            Yaml::Seq(_) => self.strings(path, v),
            Yaml::Map(deps) => {
                let mut out = Vec::new();
                for (dep, options) in deps {
                    let at = format!("{path}.{dep}");
                    if let Some(options) = self.map_or_empty(&at, options)? {
                        for (key, value) in options {
                            match (key.as_str(), value) {
                                ("condition", Yaml::Str(c)) if c == "service_started" => {}
                                ("condition", Yaml::Str(c)) => self.warn(
                                    &format!("{at}.condition"),
                                    &format!(
                                        "'{c}' is not supported, only the start order is kept"
                                    ),
                                ),
                                _ => self.unsupported(&format!("{at}.{key}")),
                            }
                        }
                    }
                    out.push(dep.clone());
                }
                Ok(out)
            }
            _ => Err(ConvertError::Shape {
                path: path.to_owned(),
                expected: "a list or a mapping",
            }),
        }
    }

    /// Service networks, without `default` (reported separately).
    fn service_networks(
        &mut self,
        path: &str,
        v: &Yaml,
    ) -> Result<(Vec<String>, bool), ConvertError> {
        let names: Vec<String> = match v {
            Yaml::Seq(_) => self.strings(path, v)?,
            Yaml::Map(entries) => {
                for (name, options) in entries {
                    let at = format!("{path}.{name}");
                    if let Some(options) = self.map_or_empty(&at, options)? {
                        for key in options.keys() {
                            self.unsupported(&format!("{at}.{key}"));
                        }
                    }
                }
                entries.keys().cloned().collect()
            }
            _ => {
                return Err(ConvertError::Shape {
                    path: path.to_owned(),
                    expected: "a list or a mapping",
                })
            }
        };
        let joins_default = names.iter().any(|n| n == "default");
        let rest = names.into_iter().filter(|n| n != "default").collect();
        Ok((rest, joins_default))
    }

    fn service(&mut self, name: &str, raw: &Map) -> Result<(Service, bool), ConvertError> {
        let mut svc = Service::default();
        let mut entrypoint = Vec::new();
        let mut joins_default = !raw.contains_key("networks");
        let mut has_image = false;
        for (key, value) in raw {
            let at = format!("services.{name}.{key}");
            match key.as_str() {
                "image" => {
                    svc.image = self.require_scalar(&at, value)?;
                    has_image = true;
                }
                "command" => svc.cmd = self.command(&at, value)?,
                "entrypoint" => entrypoint = self.command(&at, value)?,
                "environment" => {
                    svc.environment = self.key_values(
                        &at,
                        value,
                        "has no value (compose takes it from the host environment), skipped",
                    )?;
                }
                "labels" => svc.labels = self.key_values(&at, value, "has no value, skipped")?,
                "ports" => svc.ports = self.ports(&at, value)?,
                "volumes" => svc.volumes = self.volumes(&at, value)?,
                "depends_on" => svc.depends_on = self.depends_on(&at, value)?,
                "networks" => {
                    let (networks, default) = self.service_networks(&at, value)?;
                    svc.networks = networks;
                    joins_default = default;
                }
                "user" => svc.user = Some(self.require_scalar(&at, value)?),
                "working_dir" => svc.workdir = Some(self.require_scalar(&at, value)?),
                k if k.starts_with("x-") => {}
                _ => self.unsupported(&at),
            }
        }
        if !has_image {
            return Err(ConvertError::NoImage {
                service: name.to_owned(),
                build: raw.contains_key("build"),
            });
        }
        // oci-containers takes one entrypoint word; the rest of a compose list are arguments.
        if let Some((first, rest)) = entrypoint.split_first() {
            svc.entrypoint = Some(first.clone());
            let mut cmd = rest.to_vec();
            cmd.append(&mut svc.cmd);
            svc.cmd = cmd;
        } else if raw.contains_key("entrypoint") {
            self.warn(
                &format!("services.{name}.entrypoint"),
                "an empty entrypoint (reset) is not supported, ignored",
            );
        }
        Ok((svc, joins_default))
    }

    fn volume(&mut self, name: &str, v: &Yaml) -> Result<Volume, ConvertError> {
        let path = format!("volumes.{name}");
        let mut volume = Volume::default();
        let Some(entries) = self.map_or_empty(&path, v)? else {
            return Ok(volume);
        };
        for (key, value) in entries {
            let at = format!("{path}.{key}");
            match (key.as_str(), value) {
                ("external", Yaml::Bool(b)) => volume.external = *b,
                ("driver", _) => volume.driver = Some(self.require_scalar(&at, value)?),
                _ => self.unsupported(&at),
            }
        }
        Ok(volume)
    }

    fn network(&mut self, name: &str, v: &Yaml) -> Result<Network, ConvertError> {
        let path = format!("networks.{name}");
        let mut network = Network::default();
        let Some(entries) = self.map_or_empty(&path, v)? else {
            return Ok(network);
        };
        for (key, value) in entries {
            let at = format!("{path}.{key}");
            match (key.as_str(), value) {
                ("external", Yaml::Bool(b)) => network.external = *b,
                ("internal", Yaml::Bool(b)) => network.internal = *b,
                ("driver", _) => {
                    let driver = self.require_scalar(&at, value)?;
                    if driver != "bridge" {
                        self.warn(&at, &format!("driver '{driver}' is not supported, ignored"));
                    }
                }
                ("ipam", _) => network.subnet = self.ipam(&at, value)?,
                _ => self.unsupported(&at),
            }
        }
        Ok(network)
    }

    /// The subnet of the first `ipam.config` entry; everything else is reported.
    fn ipam(&mut self, path: &str, v: &Yaml) -> Result<Option<String>, ConvertError> {
        let mut subnet = None;
        let Some(entries) = self.map_or_empty(path, v)? else {
            return Ok(None);
        };
        for (key, value) in entries {
            let at = format!("{path}.{key}");
            if key != "config" {
                self.unsupported(&at);
                continue;
            }
            for (i, config) in self.seq(&at, value)?.iter().enumerate() {
                let at = format!("{at}[{i}]");
                for (key, value) in self.map(&at, config)? {
                    let field = format!("{at}.{key}");
                    if key == "subnet" && subnet.is_none() {
                        subnet = Some(self.require_scalar(&field, value)?);
                    } else {
                        self.unsupported(&field);
                    }
                }
            }
        }
        Ok(subnet)
    }

    fn project(
        &mut self,
        doc: &Map,
        name: Option<&str>,
        fallback: Option<&str>,
    ) -> Result<Project, ConvertError> {
        let file_name = match doc.get("name") {
            Some(v) => Some(self.require_scalar("name", v)?),
            None => None,
        };
        let name = name
            .map(str::to_owned)
            .or(file_name)
            .or_else(|| fallback.map(str::to_owned))
            .ok_or(ConvertError::NoName)?;
        if !valid_name(&name) {
            return Err(ConvertError::Name {
                kind: "project",
                name,
            });
        }
        let mut project = Project {
            name,
            ..Project::default()
        };

        for key in doc.keys() {
            match key.as_str() {
                "name" | "services" | "volumes" | "networks" => {}
                // `version` is obsolete and ignored by compose itself; `x-*` hold anchors.
                "version" => {}
                k if k.starts_with("x-") => {}
                _ => self.unsupported(key),
            }
        }

        let services = match doc.get("services") {
            Some(v) => self.map("services", v)?,
            None => {
                return Err(ConvertError::Shape {
                    path: "services".to_owned(),
                    expected: "a mapping",
                })
            }
        };
        if let Some(v) = doc.get("volumes") {
            for (name, entry) in self.map("volumes", v)? {
                if !valid_name(name) {
                    return Err(ConvertError::Name {
                        kind: "volume",
                        name: name.clone(),
                    });
                }
                project
                    .volumes
                    .insert(name.clone(), self.volume(name, entry)?);
            }
        }
        if let Some(v) = doc.get("networks") {
            for (name, entry) in self.map("networks", v)? {
                if name == "default" {
                    self.warn(
                        "networks.default",
                        "customizing the default network is not supported, ignored",
                    );
                    continue;
                }
                if !valid_name(name) {
                    return Err(ConvertError::Name {
                        kind: "network",
                        name: name.clone(),
                    });
                }
                project
                    .networks
                    .insert(name.clone(), self.network(name, entry)?);
            }
        }

        let mut joining = BTreeMap::new();
        for (name, raw) in services {
            if !valid_name(name) {
                return Err(ConvertError::Name {
                    kind: "service",
                    name: name.clone(),
                });
            }
            let raw = self.map(&format!("services.{name}"), raw)?;
            let (service, joins_default) = self.service(name, raw)?;
            joining.insert(name.clone(), joins_default);
            project.services.insert(name.clone(), service);
        }

        self.cross_check(&mut project, &joining);
        Ok(project)
    }

    /// Checks that span services, volumes and networks.
    fn cross_check(&mut self, project: &mut Project, joining: &BTreeMap<String, bool>) {
        project.default_network = joining.values().any(|joins| *joins);
        if project.default_network {
            for (service, joins) in joining {
                if !joins {
                    self.warn(
                        &format!("services.{service}.networks"),
                        "pin-oci attaches every service to the project's default network, \
                         compose does not attach this one",
                    );
                }
            }
        }
        for (name, service) in &project.services {
            for dep in &service.depends_on {
                if !project.services.contains_key(dep) {
                    self.warn(
                        &format!("services.{name}.depends_on"),
                        &format!("'{dep}' is not a service of the file"),
                    );
                }
            }
            for network in &service.networks {
                if !project.networks.contains_key(network) {
                    self.warn(
                        &format!("services.{name}.networks"),
                        &format!("'{network}' is not declared under top-level networks"),
                    );
                }
            }
            for volume in service.volumes.iter().filter_map(|v| named_volume(v)) {
                if !project.volumes.contains_key(volume) {
                    self.warn(
                        &format!("services.{name}.volumes"),
                        &format!("'{volume}' is not declared under top-level volumes"),
                    );
                }
            }
        }
    }
}

/// Converts a compose document. `name` (from `--name`) wins over the file's `name:`, which wins
/// over `fallback` (the directory name); `source` only labels the generated header.
pub(crate) fn convert(
    yaml: &str,
    name: Option<&str>,
    fallback: Option<&str>,
    source: &str,
) -> Result<Converted, ConvertError> {
    // Compose reads YAML 1.2 booleans: only `true` and `false`. Without this, `no`, `yes`, `on`
    // and `n` become booleans, so `DEBUG: no` would be written as "false".
    let mut options = serde_saphyr::Options::default();
    options.strict_booleans = true;
    let doc: Map = serde_saphyr::from_str_with_options(yaml, options).map_err(|e| {
        let mut message = e.to_string();
        if message.contains("untagged enum Yaml") {
            message.push_str(
                "\nhint: keys must be strings; quote a key that looks like a number or a boolean",
            );
        }
        ConvertError::Yaml(message)
    })?;
    let mut converter = Converter::default();
    let project = converter.project(&doc, name, fallback)?;
    if !converter.interpolated.is_empty() || !converter.numbers.is_empty() {
        let mut interpolated = converter.interpolated;
        let mut numbers = converter.numbers;
        interpolated.sort();
        numbers.sort();
        return Err(ConvertError::Values {
            interpolated,
            numbers,
        });
    }
    Ok(Converted {
        nix: render::nix(&project, source),
        warnings: converter.warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(yaml: &str) -> Converted {
        convert(yaml, Some("p"), None, "x.yml").unwrap()
    }

    fn err(yaml: &str) -> ConvertError {
        convert(yaml, Some("p"), None, "x.yml").unwrap_err()
    }

    fn has_warning(c: &Converted, needle: &str) -> bool {
        c.warnings.iter().any(|w| w.contains(needle))
    }

    #[test]
    fn minimal_service() {
        let c = conv("services:\n  web:\n    image: caddy:2-alpine\n");
        assert!(c.warnings.is_empty());
        assert!(c.nix.contains("image = \"caddy:2-alpine\";"));
        assert!(c.nix.contains("networks.default.enable = true;"));
    }

    #[test]
    fn environment_in_list_and_map_forms_are_equivalent() {
        let list = conv("services:\n  a:\n    image: i\n    environment: [\"A=1\", \"B=two=2\"]\n");
        let map =
            conv("services:\n  a:\n    image: i\n    environment:\n      A: 1\n      B: two=2\n");
        assert_eq!(list.nix, map.nix);
        assert!(list.nix.contains("\"B\" = \"two=2\";"));
    }

    #[test]
    fn environment_without_a_value_is_reported_not_invented() {
        let c = conv("services:\n  a:\n    image: i\n    environment: [\"FROM_HOST\", \"K=v\"]\n    labels:\n      empty:\n");
        assert!(has_warning(&c, "services.a.environment.FROM_HOST"));
        assert!(has_warning(&c, "services.a.labels.empty"));
        assert!(!c.nix.contains("FROM_HOST"));
    }

    #[test]
    fn command_string_is_split_like_a_shell() {
        let c = conv("services:\n  a:\n    image: i\n    command: sh -c \"echo 'a b'\" --x=\\ y\n");
        assert!(c
            .nix
            .contains("\"sh\"\n        \"-c\"\n        \"echo 'a b'\"\n        \"--x= y\""));
        assert_eq!(
            shell_words("a \"\" 'b c' d\\ e").unwrap(),
            ["a", "", "b c", "d e"]
        );
        assert!(matches!(
            err("services:\n  a:\n    image: i\n    command: \"echo 'oops\"\n"),
            ConvertError::Command { .. }
        ));
    }

    #[test]
    fn entrypoint_list_becomes_one_entrypoint_and_leading_arguments() {
        let c = conv("services:\n  a:\n    image: i\n    entrypoint: [\"/bin/app\", \"--serve\"]\n    command: [\"--port\", \"80\"]\n");
        assert!(c.nix.contains("entrypoint = \"/bin/app\";"));
        let cmd = c.nix.find("cmd = [").unwrap();
        let serve = c.nix.find("\"--serve\"").unwrap();
        let port = c.nix.find("\"--port\"").unwrap();
        assert!(
            cmd < serve && serve < port,
            "entrypoint arguments come first"
        );
    }

    #[test]
    fn ports_short_and_long_syntax() {
        let c = conv(
            "services:\n  a:\n    image: i\n    ports:\n      - \"80:80\"\n      - 8080\n      - target: 53\n        published: 5353\n        protocol: udp\n        host_ip: 127.0.0.1\n      - target: 443\n        published: 8443\n        protocol: tcp\n",
        );
        for port in [
            "\"80:80\"",
            "\"8080\"",
            "\"127.0.0.1:5353:53/udp\"",
            "\"8443:443\"",
        ] {
            assert!(c.nix.contains(port), "{port}\n{}", c.nix);
        }
    }

    #[test]
    fn volumes_named_bind_long_and_tmpfs() {
        let c = conv(
            "services:\n  a:\n    image: i\n    volumes:\n      - data:/d\n      - /srv:/s:ro\n      - type: volume\n        source: data\n        target: /e\n        read_only: true\n      - type: tmpfs\n        target: /t\nvolumes:\n  data:\n",
        );
        for entry in ["\"data:/d\"", "\"/srv:/s:ro\"", "\"data:/e:ro\""] {
            assert!(c.nix.contains(entry), "{entry}");
        }
        assert!(c.nix.contains("volumes.\"data\" = { };"));
        assert!(has_warning(&c, "tmpfs mounts are not supported"));
        assert!(!c.nix.contains("/t\""));
    }

    #[test]
    fn relative_bind_mounts_are_flagged() {
        let c = conv("services:\n  a:\n    image: i\n    volumes:\n      - ./data:/d\n");
        assert!(c.nix.contains("\"./data:/d\""));
        assert!(has_warning(&c, "relative bind mount"));
    }

    #[test]
    fn undeclared_volumes_networks_and_dependencies_are_flagged() {
        let c = conv(
            "services:\n  a:\n    image: i\n    volumes: [\"ghost:/g\"]\n    networks: [lost]\n    depends_on: [nobody]\n",
        );
        assert!(has_warning(
            &c,
            "'ghost' is not declared under top-level volumes"
        ));
        assert!(has_warning(
            &c,
            "'lost' is not declared under top-level networks"
        ));
        assert!(has_warning(&c, "'nobody' is not a service of the file"));
    }

    #[test]
    fn depends_on_conditions_other_than_started_are_reported() {
        let c = conv(
            "services:\n  db:\n    image: i\n  a:\n    image: i\n    depends_on:\n      db:\n        condition: service_healthy\n",
        );
        assert!(c.nix.contains("dependsOn = [\n        \"db\"\n      ];"));
        assert!(has_warning(
            &c,
            "'service_healthy' is not supported, only the start order is kept"
        ));
        let ok = conv(
            "services:\n  db:\n    image: i\n  a:\n    image: i\n    depends_on:\n      db:\n        condition: service_started\n",
        );
        assert!(ok.warnings.is_empty());
    }

    #[test]
    fn networks_default_is_implicit_and_explicit_networks_leave_it() {
        // Every service names its own networks and none lists `default`: no default network.
        let c = conv("services:\n  a:\n    image: i\n    networks: [n]\nnetworks:\n  n:\n    internal: true\n    ipam:\n      config:\n        - subnet: 10.1.0.0/24\n");
        assert!(!c.nix.contains("networks.default"));
        assert!(c.nix.contains("internal = true;") && c.nix.contains("subnet = \"10.1.0.0/24\";"));

        // A mix: pin-oci cannot attach only some services to the default network.
        let c = conv("services:\n  a:\n    image: i\n  b:\n    image: i\n    networks: [n]\nnetworks:\n  n:\n");
        assert!(c.nix.contains("networks.default.enable = true;"));
        assert!(has_warning(&c, "services.b.networks"));
    }

    #[test]
    fn external_resources_and_drivers() {
        let c = conv("services:\n  a:\n    image: i\nvolumes:\n  v:\n    external: true\n  w:\n    driver: nfs\nnetworks:\n  n:\n    external: true\n    driver: overlay\n");
        assert!(c.nix.contains("volumes.\"v\" = {\n      external = true;"));
        assert!(c.nix.contains("driver = \"nfs\";"));
        assert!(c.nix.contains("networks.\"n\" = {\n      external = true;"));
        assert!(has_warning(
            &c,
            "networks.n.driver: driver 'overlay' is not supported"
        ));
    }

    #[test]
    fn every_unsupported_key_is_reported_at_every_level() {
        let c = conv(
            "services:\n  a:\n    image: i\n    healthcheck: {test: x}\n    restart: always\n    build: .\n    env_file: .env\n    container_name: n\n    x-note: ok\nsecrets: {}\nconfigs: {}\nx-anchor: {}\nversion: '3'\n",
        );
        for key in [
            "services.a.healthcheck",
            "services.a.restart",
            "services.a.build",
            "services.a.env_file",
            "services.a.container_name",
            "secrets",
            "configs",
        ] {
            assert!(has_warning(&c, &format!("{key}: not supported")), "{key}");
        }
        // `x-*` extensions and the obsolete `version` are not noise.
        assert!(
            !has_warning(&c, "x-note")
                && !has_warning(&c, "x-anchor")
                && !has_warning(&c, "version")
        );
    }

    #[test]
    fn a_service_without_image_is_an_error_and_names_build() {
        assert_eq!(
            err("services:\n  a:\n    build: .\n"),
            ConvertError::NoImage {
                service: "a".into(),
                build: true
            }
        );
        assert!(err("services:\n  a:\n    build: .\n")
            .to_string()
            .contains("pin-oci only runs images"));
        assert_eq!(
            err("services:\n  a:\n    ports: ['1:1']\n"),
            ConvertError::NoImage {
                service: "a".into(),
                build: false
            }
        );
    }

    #[test]
    fn interpolation_is_an_error_listing_every_value() {
        let e = err(
            "services:\n  a:\n    image: nginx:${TAG:-1}\n    environment:\n      HOME: $HOME\n      OK: cost $$5\n    command: [\"--x=${X}\"]\n",
        );
        let ConvertError::Values {
            interpolated,
            numbers,
        } = &e
        else {
            panic!("expected Values, got {e}");
        };
        assert_eq!(
            interpolated,
            &[
                "services.a.command[0]",
                "services.a.environment.HOME",
                "services.a.image"
            ]
        );
        assert!(numbers.is_empty());
        assert!(e.to_string().contains("  - services.a.image"));
    }

    #[test]
    fn yaml_1_1_booleans_stay_the_text_that_was_written() {
        // `no`, `yes`, `on`, `n` are plain strings for compose; `true`/`false` are booleans.
        let c = conv(
            "services:\n  a:\n    image: i\n    environment:\n      A: no\n      B: yes\n      C: on\n      D: n\n      E: true\n      F: False\n",
        );
        for (key, value) in [
            ("A", "no"),
            ("B", "yes"),
            ("C", "on"),
            ("D", "n"),
            ("E", "true"),
            ("F", "false"),
        ] {
            assert!(
                c.nix.contains(&format!("\"{key}\" = \"{value}\";")),
                "{key}\n{}",
                c.nix
            );
        }
    }

    #[test]
    fn keys_that_look_like_booleans_are_ordinary_names() {
        let c =
            conv("services:\n  n:\n    image: i\n    environment:\n      Y: '1'\n      N: '2'\n");
        assert!(c.nix.contains("services.\"n\" = {") && c.nix.contains("\"Y\" = \"1\";"));
    }

    #[test]
    fn unquoted_numbers_that_would_be_rewritten_are_an_error() {
        // Integers are safe. 1.10, 007 and 1e3 reach us as floats, already altered.
        let ok = conv(
            "services:\n  a:\n    image: i\n    environment:\n      PORT: 8080\n      N: -3\n",
        );
        assert!(ok.nix.contains("\"PORT\" = \"8080\";") && ok.nix.contains("\"N\" = \"-3\";"));

        let e = err(
            "services:\n  a:\n    image: i\n    environment:\n      VERSION: 1.10\n      ZIP: 007\n      OK: '1.10'\n    command: [--ratio, 0.50]\n",
        );
        let ConvertError::Values {
            numbers,
            interpolated,
        } = &e
        else {
            panic!("expected Values, got {e}");
        };
        assert_eq!(
            numbers,
            &[
                "services.a.command[1]",
                "services.a.environment.VERSION",
                "services.a.environment.ZIP"
            ]
        );
        assert!(interpolated.is_empty());
        assert!(e.to_string().contains("quote them"));
    }

    #[test]
    fn both_kinds_of_unfaithful_values_are_reported_together() {
        let e =
            err("services:\n  a:\n    image: i\n    environment:\n      A: $X\n      B: 1.10\n");
        let text = e.to_string();
        assert!(text.contains("variable interpolation") && text.contains("unquoted numbers"));
        assert!(
            text.contains("services.a.environment.A") && text.contains("services.a.environment.B")
        );
    }

    #[test]
    fn escaped_dollars_become_literal_dollars() {
        let c = conv("services:\n  a:\n    image: i\n    environment:\n      PRICE: cost $$5\n");
        assert!(c.nix.contains("\"PRICE\" = \"cost $5\";"));
    }

    #[test]
    fn hostile_values_stay_inert_strings() {
        // Written with $$ to get past the interpolation check, which is itself a guard:
        // the generated module must never evaluate a compose value as Nix.
        let c = conv("services:\n  a:\n    image: i\n    environment:\n      X: \"$${builtins.exec [\\\"id\\\"]}\"\n      Y: \"it's ''quoted'' \\\\ \\\"x\\\"\"\n");
        assert!(
            c.nix.contains(r#""X" = "\${builtins.exec [\"id\"]}";"#),
            "{}",
            c.nix
        );
        assert!(
            c.nix.contains(r#""Y" = "it's ''quoted'' \\ \"x\"";"#),
            "{}",
            c.nix
        );
    }

    #[test]
    fn anchors_and_merge_keys_are_resolved() {
        let c = conv(
            "x-common: &common\n  user: nobody\n  environment:\n    A: '1'\nservices:\n  a:\n    <<: *common\n    image: i\n  b:\n    <<: *common\n    image: j\n    user: root\n",
        );
        assert!(c.nix.contains("\"a\" = {\n      image = \"i\";"));
        assert_eq!(c.nix.matches("user = \"nobody\";").count(), 1);
        assert!(
            c.nix.contains("user = \"root\";"),
            "an explicit key beats the merged one"
        );
        assert_eq!(c.nix.matches("\"A\" = \"1\";").count(), 2);
    }

    #[test]
    fn project_name_comes_from_the_flag_then_the_file_then_the_directory() {
        let yaml = "name: from-file\nservices:\n  a:\n    image: i\n";
        let pick = |flag, fallback| {
            convert(yaml, flag, fallback, "x")
                .unwrap()
                .nix
                .lines()
                .nth(3)
                .unwrap()
                .to_owned()
        };
        assert!(pick(Some("flag"), Some("dir")).contains("projects.\"flag\""));
        assert!(pick(None, Some("dir")).contains("projects.\"from-file\""));
        let bare = "services:\n  a:\n    image: i\n";
        let c = convert(bare, None, Some("dir"), "x").unwrap();
        assert!(c.nix.contains("projects.\"dir\""));
        assert_eq!(
            convert(bare, None, None, "x").unwrap_err(),
            ConvertError::NoName
        );
    }

    #[test]
    fn names_are_checked_like_the_module_does() {
        for (yaml, kind) in [
            ("services:\n  'a b':\n    image: i\n", "service"),
            (
                "services:\n  a:\n    image: i\nvolumes:\n  '-x':\n",
                "volume",
            ),
            (
                "services:\n  a:\n    image: i\nnetworks:\n  'n/m':\n",
                "network",
            ),
        ] {
            assert!(
                matches!(err(yaml), ConvertError::Name { kind: k, .. } if k == kind),
                "{kind}"
            );
        }
        assert!(matches!(
            convert(
                "services:\n  a:\n    image: i\n",
                Some("bad name"),
                None,
                "x"
            ),
            Err(ConvertError::Name {
                kind: "project",
                ..
            })
        ));
    }

    #[test]
    fn malformed_input_is_an_error_with_a_position() {
        let e = err("services:\n  a:\n    image: [\n");
        assert!(
            matches!(&e, ConvertError::Yaml(m) if m.contains("line 3")),
            "{e}"
        );
        assert!(matches!(err("services: []\n"), ConvertError::Shape { .. }));
        assert!(matches!(err("version: '3'\n"), ConvertError::Shape { .. }));
        assert!(matches!(
            err("services:\n  a:\n    image: i\n    ports: {a: 1}\n"),
            ConvertError::Shape { .. }
        ));
    }

    #[test]
    fn conversion_is_deterministic() {
        let yaml =
            "services:\n  z:\n    image: i\n    environment: {B: 1, A: 2}\n  a:\n    image: j\n";
        assert_eq!(conv(yaml).nix, conv(yaml).nix);
        assert!(
            conv(yaml).nix.find("\"a\" = {").unwrap() < conv(yaml).nix.find("\"z\" = {").unwrap()
        );
    }
}

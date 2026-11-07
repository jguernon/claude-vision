//! Lecture incrémentale des transcripts JSONL et conversion des lignes en
//! événements de flux. Seuls les octets ajoutés depuis le dernier appel sont lus.
use crate::i18n::t;
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use memchr::memmem::Finder;
use serde_json::Value;

use crate::util::{now_ms, preview};

pub struct Tailer {
    pub path: PathBuf,
    pub offset: u64,
    partial: Vec<u8>,
}

impl Tailer {
    /// Ouvre le fichier `tail_bytes` avant la fin (la première ligne partielle est ignorée).
    pub fn open(path: &Path, tail_bytes: u64) -> Self {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let mut t = Tailer {
            path: path.to_path_buf(),
            offset: 0,
            partial: Vec::new(),
        };
        if size > tail_bytes {
            t.offset = size - tail_bytes;
            if let Ok(mut f) = File::open(path) {
                if f.seek(SeekFrom::Start(t.offset)).is_ok() {
                    let mut r = BufReader::new(f);
                    let mut skip = Vec::new();
                    if let Ok(n) = r.read_until(b'\n', &mut skip) {
                        t.offset += n as u64;
                    }
                }
            }
        }
        t
    }

    pub fn size(&self) -> u64 {
        std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0)
    }

    /// Nouvelles lignes complètes depuis le dernier appel.
    pub fn poll(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        let size = self.size();
        if size < self.offset {
            // fichier tronqué ou recréé
            self.offset = 0;
            self.partial.clear();
        }
        if size == self.offset {
            return out;
        }
        let Ok(mut f) = File::open(&self.path) else {
            return out;
        };
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return out;
        }
        let mut buf = Vec::with_capacity((size - self.offset) as usize);
        if (&mut f).take(size - self.offset).read_to_end(&mut buf).is_err() {
            return out;
        }
        self.offset = size;
        self.partial.extend_from_slice(&buf);
        let mut start = 0;
        while let Some(pos) = memchr::memchr(b'\n', &self.partial[start..]) {
            let line = &self.partial[start..start + pos];
            if !line.is_empty() {
                out.push(String::from_utf8_lossy(line).into_owned());
            }
            start += pos + 1;
        }
        self.partial.drain(..start);
        out
    }
}

#[derive(Default, Clone, Debug)]
pub struct Counts {
    pub prompts: u32,
    pub tools: u32,
    pub assistant: u32,
    /// dernier prompt humain : (texte, horodatage)
    pub last_prompt: Option<(String, i64)>,
}

/// Compte rapidement (sans parser le JSON) les prompts et appels d'outils d'un transcript.
pub fn quick_counts(path: &Path) -> Counts {
    let mut c = Counts::default();
    let Ok(f) = File::open(path) else {
        return c;
    };
    let mut r = BufReader::with_capacity(1 << 20, f);
    let mut line = Vec::new();
    let mut last_line: Vec<u8> = Vec::new();
    let f_asst = Finder::new(b"\"type\":\"assistant\"");
    let f_role_asst = Finder::new(b"\"role\":\"assistant\"");
    let f_user = Finder::new(b"\"type\":\"user\"");
    let f_tres = Finder::new(b"\"type\":\"tool_result\"");
    let f_tuse = Finder::new(b"\"type\":\"tool_use\"");
    let f_side = Finder::new(b"\"isSidechain\":true");
    let f_meta = Finder::new(b"\"isMeta\":true");
    const SKIP: [&[u8]; 8] = [
        b"<agent-message",
        b"Another Claude session sent",
        b"<task-notification>",
        b"-event>",
        b"<system-reminder>",
        b"[Request interrupted",
        b"<local-command-",
        b"<command-name>",
    ];
    let skip: Vec<Finder> = SKIP.iter().map(|n| Finder::new(n)).collect();
    loop {
        line.clear();
        match r.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            _ => {}
        }
        if f_asst.find(&line).is_some() && f_role_asst.find(&line).is_some() {
            c.assistant += 1;
            if f_tuse.find(&line).is_some() {
                c.tools += 1;
            }
        } else if f_user.find(&line).is_some()
            && f_side.find(&line).is_none()
            && f_meta.find(&line).is_none()
            && f_tres.find(&line).is_none()
            && skip.iter().all(|f| f.find(&line).is_none())
        {
            c.prompts += 1;
            last_line.clear();
            last_line.extend_from_slice(&line);
        }
    }
    if !last_line.is_empty() {
        if let Ok(v) = serde_json::from_slice::<Value>(&last_line) {
            let text = text_of(&v["message"]["content"]);
            let ts = v["timestamp"].as_str().and_then(parse_ts).unwrap_or(0);
            if !text.trim().is_empty() {
                c.last_prompt = Some((preview(&tidy_prompt(&text), 600), ts));
            }
        }
    }
    c
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// de la session vers un outil ou un agent
    Out,
    /// vers la session (résultat, notification, réponse du modèle, prompt)
    In,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Prompt,
    Tool,
    Result,
    Agent,
    Notif,
    Think,
    Text,
    Error,
    System,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Prompt => "prompt",
            Kind::Tool => t("outil", "tool"),
            Kind::Result => t("résultat", "result"),
            Kind::Agent => "Agent",
            Kind::Notif => "notif",
            Kind::Think => t("réflexion", "thinking"),
            Kind::Text => t("texte", "text"),
            Kind::Error => t("erreur", "error"),
            Kind::System => t("système", "system"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RawEvent {
    pub ts_ms: i64,
    pub dir: Dir,
    pub kind: Kind,
    pub name: String,
    pub bytes: u64,
    pub tokens: Option<u64>,
    /// identifiant d'appel reliant un lancement ou un retour à son agent
    pub link: Option<String>,
    pub note: String,
}

pub enum Item {
    /// effort et modèle conseiller de la session
    Meta {
        effort: Option<String>,
        advisor: Option<String>,
    },
    Event(RawEvent),
    Model(String),
    /// taille du contexte (entrée + cache) du dernier appel au modèle
    Ctx(u64),
    Prompt(String),
    AgentLaunch {
        tool_use_id: String,
        bytes: u64,
        ts_ms: i64,
    },
    AgentDone {
        tool_use_id: String,
        bytes: u64,
        summary: String,
        ts_ms: i64,
    },
}

fn parse_ts(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp_millis())
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(|x| x["text"].as_str())
            .collect::<Vec<_>>()
            .join(" "),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn tag(text: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let a = text.find(&open)? + open.len();
    let b = text[a..].find(&close)? + a;
    Some(text[a..b].trim().to_string())
}

/// Remplace les références d'images collées (« [Image: source: /chemin] ») par « [image] ».
pub fn tidy_prompt(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("[Image") {
        out.push_str(&rest[..i]);
        match rest[i..].find(']') {
            Some(j) => {
                out.push_str("[image]");
                rest = &rest[i + j + 1..];
            }
            None => {
                rest = &rest[i..];
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Retire les balises `<...>` d'un texte.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '<' => depth += 1,
            '>' if depth > 0 => {
                depth -= 1;
                out.push(' ');
            }
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

pub fn short_tool(n: &str) -> String {
    if let Some(rest) = n.strip_prefix("mcp__") {
        rest.rsplit("__").next().unwrap_or(rest).to_string()
    } else {
        n.to_string()
    }
}

fn short_path(p: &str) -> String {
    let parts: Vec<&str> = p.rsplit('/').take(2).collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}

fn tool_note(name: &str, input: &Value) -> String {
    let pick = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| input[*k].as_str().map(|s| s.to_string()))
    };
    let s = match name {
        "Bash" => pick(&["description", "command"]),
        "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            pick(&["file_path", "notebook_path"]).map(|p| short_path(&p))
        }
        "Grep" | "Glob" => pick(&["pattern"]),
        "WebFetch" => pick(&["url"]),
        "WebSearch" => pick(&["query"]),
        "Skill" => pick(&["skill"]),
        _ => input
            .as_object()
            .and_then(|o| o.values().find_map(|v| v.as_str().map(|s| s.to_string()))),
    };
    preview(&s.unwrap_or_default(), 80)
}

/// Convertit une ligne de transcript en éléments. `names` mémorise les noms
/// d'outils par identifiant d'appel pour nommer les résultats.
pub fn parse_line(line: &str, names: &mut HashMap<String, String>) -> Vec<Item> {
    let mut items = Vec::new();
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return items;
    };
    let ty = v["type"].as_str().unwrap_or("");
    let ts_ms = v["timestamp"]
        .as_str()
        .and_then(parse_ts)
        .unwrap_or_else(now_ms);
    match ty {
        "assistant" => {
            let msg = &v["message"];
            if let Some(m) = msg["model"].as_str() {
                items.push(Item::Model(m.to_string()));
            }
            let effort = v["effort"].as_str().map(|x| x.to_string());
            let advisor = v["advisorModel"].as_str().map(|x| x.to_string());
            if effort.is_some() || advisor.is_some() {
                items.push(Item::Meta { effort, advisor });
            }
            let u = &msg["usage"];
            let out_tokens = u["output_tokens"].as_u64();
            let think_tokens = u["output_tokens_details"]["thinking_tokens"].as_u64();
            if u.is_object() {
                let ctx = u["input_tokens"].as_u64().unwrap_or(0)
                    + u["cache_read_input_tokens"].as_u64().unwrap_or(0)
                    + u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
                if ctx > 0 {
                    items.push(Item::Ctx(ctx));
                }
            }
            let Some(blocks) = msg["content"].as_array() else {
                return items;
            };
            for b in blocks {
                match b["type"].as_str().unwrap_or("") {
                    "thinking" => {
                        let len = b["thinking"].as_str().map(|s| s.len()).unwrap_or(0) as u64;
                        items.push(Item::Event(RawEvent {
                            ts_ms,
                            dir: Dir::In,
                            kind: Kind::Think,
                            name: t("modèle", "model").into(),
                            bytes: len,
                            tokens: think_tokens.or(out_tokens),
                            link: None,
                            note: String::new(),
                        }));
                    }
                    "text" => {
                        let s = b["text"].as_str().unwrap_or("");
                        items.push(Item::Event(RawEvent {
                            ts_ms,
                            dir: Dir::In,
                            kind: Kind::Text,
                            name: t("modèle", "model").into(),
                            bytes: s.len() as u64,
                            tokens: out_tokens,
                            link: None,
                            note: preview(s, 80),
                        }));
                    }
                    "tool_use" => {
                        let id = b["id"].as_str().unwrap_or("").to_string();
                        let name = b["name"].as_str().unwrap_or(t("outil", "tool")).to_string();
                        let input = &b["input"];
                        let bytes = input.to_string().len() as u64;
                        if names.len() > 8192 {
                            names.clear();
                        }
                        names.insert(id.clone(), name.clone());
                        if name == "Agent" || name == "Task" {
                            let agent_type = input["subagent_type"]
                                .as_str()
                                .unwrap_or("general-purpose")
                                .to_string();
                            let desc = input["description"].as_str().unwrap_or("").to_string();
                            let plen = input["prompt"].as_str().map(|s| s.len()).unwrap_or(0) as u64;
                            items.push(Item::AgentLaunch {
                                tool_use_id: id.clone(),
                                bytes: plen.max(bytes),
                                ts_ms,
                            });
                            items.push(Item::Event(RawEvent {
                                ts_ms,
                                dir: Dir::Out,
                                kind: Kind::Agent,
                                name: agent_type,
                                bytes,
                                tokens: None,
                                link: Some(id),
                                note: desc,
                            }));
                        } else {
                            items.push(Item::Event(RawEvent {
                                ts_ms,
                                dir: Dir::Out,
                                kind: Kind::Tool,
                                name: short_tool(&name),
                                bytes,
                                tokens: None,
                                link: None,
                                note: tool_note(&name, input),
                            }));
                        }
                    }
                    _ => {}
                }
            }
        }
        "user" => {
            let content = &v["message"]["content"];
            if let Some(arr) = content.as_array() {
                let mut had_result = false;
                for b in arr {
                    if b["type"].as_str() != Some("tool_result") {
                        continue;
                    }
                    had_result = true;
                    let id = b["tool_use_id"].as_str().unwrap_or("");
                    let name = names.get(id).cloned().unwrap_or_else(|| t("outil", "tool").into());
                    let body = text_of(&b["content"]);
                    let is_err = b["is_error"].as_bool().unwrap_or(false);
                    let bytes = body.len() as u64;
                    if name == "Agent" || name == "Task" {
                        if body.starts_with("Async agent launched") {
                            continue;
                        }
                        items.push(Item::AgentDone {
                            tool_use_id: id.to_string(),
                            bytes,
                            summary: preview(&body, 80),
                            ts_ms,
                        });
                        items.push(Item::Event(RawEvent {
                            ts_ms,
                            dir: Dir::In,
                            kind: Kind::Notif,
                            name: "agent".into(),
                            bytes,
                            tokens: None,
                            link: Some(id.to_string()),
                            note: preview(&body, 80),
                        }));
                    } else {
                        items.push(Item::Event(RawEvent {
                            ts_ms,
                            dir: Dir::In,
                            kind: if is_err { Kind::Error } else { Kind::Result },
                            name: short_tool(&name),
                            bytes,
                            tokens: None,
                            link: None,
                            note: preview(&body, 80),
                        }));
                    }
                }
                if had_result {
                    return items;
                }
            }
            let text = text_of(content);
            if text.is_empty() {
                return items;
            }
            if text.contains("<task-notification>") {
                let summary = tag(&text, "summary").unwrap_or_default();
                let status = tag(&text, "status").unwrap_or_default();
                let link = tag(&text, "tool-use-id");
                items.push(Item::AgentDone {
                    tool_use_id: link.clone().unwrap_or_default(),
                    bytes: text.len() as u64,
                    summary: preview(&summary, 80),
                    ts_ms,
                });
                items.push(Item::Event(RawEvent {
                    ts_ms,
                    dir: Dir::In,
                    kind: Kind::Notif,
                    name: if status.is_empty() { t("tâche", "task").into() } else { status },
                    bytes: text.len() as u64,
                    tokens: None,
                    link,
                    note: preview(&summary, 80),
                }));
            } else if text.starts_with("Another Claude session") || text.contains("<agent-message") {
                // message d'un agent ou d'une autre session vers celle-ci : un retour, pas un prompt
                items.push(Item::Event(RawEvent {
                    ts_ms,
                    dir: Dir::In,
                    kind: Kind::Notif,
                    name: "message".into(),
                    bytes: text.len() as u64,
                    tokens: None,
                    link: None,
                    note: preview(&strip_tags(&text), 80),
                }));
            } else if text.starts_with('<') && (text.contains("-event>") || text.contains("<system-reminder>")) {
                items.push(Item::Event(RawEvent {
                    ts_ms,
                    dir: Dir::In,
                    kind: Kind::System,
                    name: "app".into(),
                    bytes: text.len() as u64,
                    tokens: None,
                    link: None,
                    note: preview(&text, 80),
                }));
            } else {
                items.push(Item::Prompt(preview(&tidy_prompt(&text), 600)));
                items.push(Item::Event(RawEvent {
                    ts_ms,
                    dir: Dir::In,
                    kind: Kind::Prompt,
                    name: t("humain", "human").into(),
                    bytes: text.len() as u64,
                    tokens: None,
                    link: None,
                    note: preview(&text, 80),
                }));
            }
        }
        _ => {}
    }
    items
}

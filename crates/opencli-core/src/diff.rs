//! What a spec refresh changes from the user's point of view: the commands
//! and arguments they script against, not the raw document.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::{ArgumentSpec, CommandSet, CommandSpec, HttpMethod, ValueType};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CommandDiff {
    pub added: Vec<CommandChange>,
    pub removed: Vec<CommandChange>,
    pub renamed: Vec<CommandRename>,
    pub changed: Vec<CommandUpdate>,
    pub schemes_added: Vec<String>,
    pub schemes_removed: Vec<String>,
    /// Same name, different type (e.g. API key → OAuth2); stored credentials no longer apply.
    pub schemes_changed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandChange {
    pub command: String,
    /// `METHOD /path`
    pub operation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandRename {
    pub from: String,
    pub to: String,
    pub operation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandUpdate {
    pub command: String,
    pub operation: String,
    pub details: Vec<String>,
    /// Existing invocations may now fail (removed or newly required inputs).
    pub breaking: bool,
}

impl CommandDiff {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// Whether scripts written against the old version may stop working.
    #[must_use]
    pub fn is_breaking(&self) -> bool {
        !self.removed.is_empty()
            || !self.renamed.is_empty()
            || !self.schemes_removed.is_empty()
            || !self.schemes_changed.is_empty()
            || self.changed.iter().any(|update| update.breaking)
    }
}

type OperationKey<'a> = (HttpMethod, &'a str);

/// Compares commands by operation (method and path), so a renamed command is
/// reported as a rename rather than a removal plus an addition.
#[must_use]
pub fn diff_commands(old: &CommandSet, new: &CommandSet) -> CommandDiff {
    let (before, after) = (by_operation(old), by_operation(new));
    let mut diff = CommandDiff::default();

    for (key, command) in &after {
        let Some(previous) = before.get(key) else {
            diff.added.push(change(command));
            continue;
        };
        if previous.path != command.path {
            diff.renamed.push(CommandRename {
                from: previous.path.join(" "),
                to: command.path.join(" "),
                operation: operation(command),
            });
        }
        let (details, breaking) = compare(previous, command);
        if !details.is_empty() {
            diff.changed.push(CommandUpdate {
                command: command.path.join(" "),
                operation: operation(command),
                details,
                breaking,
            });
        }
    }
    diff.removed = before
        .iter()
        .filter(|(key, _)| !after.contains_key(*key))
        .map(|(_, command)| change(command))
        .collect();

    let (old_schemes, new_schemes) = (&old.security_schemes, &new.security_schemes);
    diff.schemes_added = new_schemes
        .keys()
        .filter(|name| !old_schemes.contains_key(*name))
        .cloned()
        .collect();
    diff.schemes_removed = old_schemes
        .keys()
        .filter(|name| !new_schemes.contains_key(*name))
        .cloned()
        .collect();
    diff.schemes_changed = new_schemes
        .iter()
        .filter(|(name, scheme)| {
            old_schemes
                .get(*name)
                .is_some_and(|previous| previous.kind() != scheme.kind())
        })
        .map(|(name, _)| name.clone())
        .collect();

    for list in [&mut diff.added, &mut diff.removed] {
        list.sort_by(|a, b| a.command.cmp(&b.command));
    }
    diff.renamed.sort_by(|a, b| a.to.cmp(&b.to));
    diff.changed.sort_by(|a, b| a.command.cmp(&b.command));
    diff
}

fn by_operation(set: &CommandSet) -> BTreeMap<OperationKey<'_>, &CommandSpec> {
    set.commands
        .iter()
        .map(|command| ((command.method, command.path_template.as_str()), command))
        .collect()
}

fn by_usage(command: &CommandSpec) -> BTreeMap<String, &ArgumentSpec> {
    command
        .arguments
        .iter()
        .map(|argument| (usage(argument), argument))
        .collect()
}

fn change(command: &CommandSpec) -> CommandChange {
    CommandChange {
        command: command.path.join(" "),
        operation: operation(command),
    }
}

fn operation(command: &CommandSpec) -> String {
    format!("{} {}", command.method, command.path_template)
}

fn compare(old: &CommandSpec, new: &CommandSpec) -> (Vec<String>, bool) {
    let mut details = Vec::new();
    let mut breaking = false;
    let mut note = |text: String, is_breaking: bool| {
        details.push(text);
        breaking |= is_breaking;
    };

    let (before, after) = (by_usage(old), by_usage(new));
    for (name, argument) in &after {
        match before.get(name) {
            None if argument.required => note(format!("new required argument {name}"), true),
            None => note(format!("new argument {name}"), false),
            Some(previous) => compare_argument(name, previous, argument, &mut note),
        }
    }
    for name in before.keys().filter(|name| !after.contains_key(*name)) {
        note(format!("argument {name} removed"), true);
    }

    match (&old.body, &new.body) {
        (None, Some(body)) if body.required => note("now requires a request body".to_owned(), true),
        (None, Some(_)) => note("now accepts a request body".to_owned(), false),
        (Some(_), None) => note("no longer accepts a request body".to_owned(), true),
        (Some(previous), Some(body)) => {
            if previous.content_type != body.content_type {
                note(
                    format!(
                        "request body type {} → {}",
                        previous.content_type, body.content_type
                    ),
                    true,
                );
            }
            if !previous.required && body.required {
                note("request body is now required".to_owned(), true);
            }
        }
        (None, None) => {}
    }
    if old.security != new.security {
        note("authentication requirements changed".to_owned(), false);
    }
    if !old.deprecated && new.deprecated {
        note("now deprecated".to_owned(), false);
    }
    (details, breaking)
}

fn compare_argument(
    name: &str,
    old: &ArgumentSpec,
    new: &ArgumentSpec,
    note: &mut impl FnMut(String, bool),
) {
    if !old.required && new.required {
        note(format!("{name} is now required"), true);
    } else if old.required && !new.required {
        note(format!("{name} is now optional"), false);
    }
    if old.value_type != new.value_type {
        note(
            format!(
                "{name} type {} → {}",
                type_name(old.value_type),
                type_name(new.value_type)
            ),
            true,
        );
    }
    if old.multiple && !new.multiple {
        note(format!("{name} no longer repeats"), true);
    }
    let (before, after): (BTreeSet<_>, BTreeSet<_>) = (
        old.allowed_values.iter().collect(),
        new.allowed_values.iter().collect(),
    );
    if before != after {
        let narrowed = !after.is_empty() && (before.is_empty() || !before.is_subset(&after));
        note(format!("{name} allowed values changed"), narrowed);
    }
}

fn usage(argument: &ArgumentSpec) -> String {
    if argument.positional {
        format!("<{}>", argument.name.to_ascii_uppercase().replace('-', "_"))
    } else {
        format!("--{}", argument.name)
    }
}

const fn type_name(value_type: ValueType) -> &'static str {
    match value_type {
        ValueType::String => "string",
        ValueType::Integer => "integer",
        ValueType::Number => "number",
        ValueType::Boolean => "boolean",
        ValueType::File => "file",
    }
}

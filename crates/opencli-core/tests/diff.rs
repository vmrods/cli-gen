// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use opencli_core::{
    ApiKeyLocation, ArgumentSpec, ArgumentTarget, BodySpec, CommandSet, CommandSpec, HttpMethod,
    SchemeRequirement, SecurityRequirement, SecurityScheme, ValueType, diff_commands,
};

fn command(path: &str, method: HttpMethod, template: &str) -> CommandSpec {
    CommandSpec {
        path: path.split(' ').map(str::to_owned).collect(),
        method,
        path_template: template.to_owned(),
        operation_id: None,
        summary: None,
        description: None,
        deprecated: false,
        arguments: Vec::new(),
        body: None,
        security: Vec::new(),
    }
}

fn flag(name: &str, required: bool, value_type: ValueType) -> ArgumentSpec {
    ArgumentSpec {
        name: name.to_owned(),
        target: ArgumentTarget::Query(name.to_owned()),
        required,
        help: None,
        value_type,
        multiple: false,
        allowed_values: Vec::new(),
        array_delimiter: None,
    }
}

fn set(commands: Vec<CommandSpec>, schemes: &[(&str, SecurityScheme)]) -> CommandSet {
    CommandSet {
        servers: Vec::new(),
        security_schemes: schemes
            .iter()
            .map(|(name, scheme)| ((*name).to_owned(), scheme.clone()))
            .collect::<BTreeMap<_, _>>(),
        commands,
    }
}

fn list() -> CommandSpec {
    command("pets list", HttpMethod::Get, "/pets")
}

fn body(required: bool, content_type: &str) -> BodySpec {
    BodySpec {
        content_type: content_type.to_owned(),
        required,
        description: None,
        schema: None,
    }
}

#[test]
fn identical_sets_have_no_differences() {
    let diff = diff_commands(&set(vec![list()], &[]), &set(vec![list()], &[]));
    assert!(diff.is_empty());
    assert!(!diff.is_breaking());
}

#[test]
fn added_operations_are_not_breaking() {
    let diff = diff_commands(
        &set(vec![list()], &[]),
        &set(
            vec![
                list(),
                command("pets search", HttpMethod::Get, "/pets/search"),
            ],
            &[],
        ),
    );
    assert_eq!(diff.added[0].command, "pets search");
    assert_eq!(diff.added[0].operation, "GET /pets/search");
    assert!(!diff.is_breaking());
}

#[test]
fn removed_operations_are_breaking() {
    let diff = diff_commands(&set(vec![list()], &[]), &set(vec![], &[]));
    assert_eq!(diff.removed[0].command, "pets list");
    assert!(diff.is_breaking());
}

#[test]
fn renamed_commands_are_reported_as_renames() {
    let renamed = command("animals list", HttpMethod::Get, "/pets");
    let diff = diff_commands(&set(vec![list()], &[]), &set(vec![renamed], &[]));
    assert_eq!(
        (diff.renamed[0].from.as_str(), diff.renamed[0].to.as_str()),
        ("pets list", "animals list")
    );
    assert!(diff.added.is_empty() && diff.removed.is_empty());
    assert!(diff.is_breaking());
}

#[test]
fn argument_changes_are_classified() {
    let mut before = list();
    before.arguments = vec![
        flag("limit", false, ValueType::Integer),
        flag("sort", false, ValueType::String),
        flag("owner", false, ValueType::String),
        flag("legacy", false, ValueType::String),
    ];
    let mut after = list();
    after.arguments = vec![
        flag("limit", false, ValueType::String),
        flag("sort", true, ValueType::String),
        flag("owner", false, ValueType::String),
        flag("page", false, ValueType::Integer),
        flag("tenant", true, ValueType::String),
    ];
    let diff = diff_commands(&set(vec![before], &[]), &set(vec![after], &[]));
    let update = &diff.changed[0];
    assert_eq!(update.command, "pets list");
    assert_eq!(
        update.details,
        [
            "--limit type integer → string",
            "new argument --page",
            "--sort is now required",
            "new required argument --tenant",
            "argument --legacy removed",
        ]
    );
    assert!(update.breaking);
}

#[test]
fn optional_additions_and_relaxations_are_not_breaking() {
    let mut before = list();
    before.arguments = vec![flag("sort", true, ValueType::String)];
    let mut after = list();
    after.arguments = vec![
        flag("sort", false, ValueType::String),
        flag("page", false, ValueType::Integer),
    ];
    let diff = diff_commands(&set(vec![before], &[]), &set(vec![after], &[]));
    assert_eq!(
        diff.changed[0].details,
        ["new argument --page", "--sort is now optional"]
    );
    assert!(!diff.is_breaking());
}

#[test]
fn narrowing_allowed_values_is_breaking_but_widening_is_not() {
    let with_values = |values: &[&str]| {
        let mut command = list();
        let mut status = flag("status", false, ValueType::String);
        status.allowed_values = values.iter().map(|&value| value.to_owned()).collect();
        command.arguments = vec![status];
        set(vec![command], &[])
    };
    assert!(
        !diff_commands(&with_values(&["open"]), &with_values(&["open", "closed"])).is_breaking()
    );
    assert!(
        diff_commands(&with_values(&["open", "closed"]), &with_values(&["open"])).is_breaking()
    );
}

#[test]
fn body_changes_are_classified() {
    let create = |body: Option<BodySpec>| {
        let mut command = command("pets create", HttpMethod::Post, "/pets");
        command.body = body;
        set(vec![command], &[])
    };
    let cases = [
        (
            None,
            Some(body(true, "application/json")),
            "now requires a request body",
            true,
        ),
        (
            None,
            Some(body(false, "application/json")),
            "now accepts a request body",
            false,
        ),
        (
            Some(body(true, "application/json")),
            None,
            "no longer accepts a request body",
            true,
        ),
        (
            Some(body(false, "application/json")),
            Some(body(true, "application/json")),
            "request body is now required",
            true,
        ),
        (
            Some(body(true, "application/json")),
            Some(body(true, "text/plain")),
            "request body type application/json → text/plain",
            true,
        ),
    ];
    for (before, after, expected, breaking) in cases {
        let diff = diff_commands(&create(before), &create(after));
        assert_eq!(diff.changed[0].details, [expected]);
        assert_eq!(diff.is_breaking(), breaking, "{expected}");
    }
}

#[test]
fn security_and_deprecation_changes_are_informational() {
    let mut after = list();
    after.deprecated = true;
    after.security = vec![SecurityRequirement {
        schemes: vec![SchemeRequirement {
            scheme: "key".into(),
            scopes: vec![],
        }],
    }];
    let diff = diff_commands(&set(vec![list()], &[]), &set(vec![after], &[]));
    assert_eq!(
        diff.changed[0].details,
        ["authentication requirements changed", "now deprecated"]
    );
    assert!(!diff.is_breaking());
}

#[test]
fn security_scheme_changes_are_reported() {
    let key = SecurityScheme::ApiKey {
        name: "X-Key".into(),
        location: ApiKeyLocation::Header,
    };
    let before = set(
        vec![],
        &[
            ("key", key.clone()),
            ("legacy", SecurityScheme::HttpBasic),
            ("token", key.clone()),
        ],
    );
    let after = set(
        vec![],
        &[
            ("key", key),
            (
                "token",
                SecurityScheme::HttpBearer {
                    bearer_format: None,
                },
            ),
            ("oauth", SecurityScheme::OAuth2 { flows: vec![] }),
        ],
    );
    let diff = diff_commands(&before, &after);
    assert_eq!(diff.schemes_added, ["oauth"]);
    assert_eq!(diff.schemes_removed, ["legacy"]);
    assert_eq!(diff.schemes_changed, ["token"]);
    assert!(diff.is_breaking());
}

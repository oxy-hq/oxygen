//! App-to-app edges: who calls whom, and who hears whose events. A second pass
//! over the org's apps, after every app has its node, because each edge names
//! its other end by slug.
//!
//! | Declared by | Edge |
//! | --- | --- |
//! | the caller's top-level `calls: { "<slug>": ["<fn>"] }` | caller app `calls` → the callee's function. One declaration admits both `ctx.apps.call` (answered now) and `ctx.apps.send` (queued, delivered later), so the edge cannot say which the code uses |
//! | a function's `on: "<slug>/<event>"` | each publishing function (its `events.publish` names the event) `publishes` → the subscribing function, the event names on the edge |
//!
//! The readers are the ones the platform acts on: `DeclaredCalls` and
//! `function_callable` (what `ctx.apps.call`/`send` admit), `EventCatalog`,
//! `function_publishes` and `function_subscribes` (what the event relay reads).
//! What the platform would refuse is still drawn, marked with the reason in
//! `problem`, because a refused declaration is what an operator is looking for:
//! `app_not_found`, `function_not_found`, `not_callable`, `not_in_catalog`,
//! `never_published`.

use std::collections::HashMap;

use oxy_custom_apps::calls::{DeclaredCalls, function_callable};
use oxy_custom_apps::events::catalog::{EventCatalog, function_publishes, function_subscribes};
use serde_json::json;

use super::EdgeKind;
use super::NodeKind;
use super::assemble::{AppIn, Graph, app_node_id, function_node_id};

/// Every calls and event edge with at least one end in an app `wanted` keeps.
/// The other end may sit in a workspace the filter leaves out; it is drawn all
/// the same, as a pipeline an upload names is. Every app is walked, because an
/// edge where the kept app is the callee or the publisher is declared by the
/// other app.
pub(super) fn add_messaging(graph: &mut Graph, apps: &[AppIn], wanted: impl Fn(&AppIn) -> bool) {
    let by_slug: HashMap<&str, &AppIn> = apps.iter().map(|a| (a.slug.as_str(), a)).collect();
    for app in apps {
        let app_wanted = wanted(app);
        add_calls(graph, &by_slug, app, app_wanted, &wanted);
        add_subscriptions(graph, &by_slug, app, app_wanted, &wanted);
    }
}

/// Every `on: "<slug>/<event>"` of the app's functions, when the subscriber or
/// the publisher is wanted.
fn add_subscriptions(
    graph: &mut Graph,
    by_slug: &HashMap<&str, &AppIn>,
    app: &AppIn,
    app_wanted: bool,
    wanted: &impl Fn(&AppIn) -> bool,
) {
    for function in &app.functions {
        let Ok(subscribed) = function_subscribes(&function.spec) else {
            continue;
        };
        for qualified in subscribed {
            let Some((slug, event)) = qualified.split_once('/') else {
                continue;
            };
            let publisher = by_slug.get(slug).copied();
            if !app_wanted && !publisher.is_some_and(wanted) {
                continue;
            }
            ensure_app(graph, app);
            let to = function_node(graph, app, &function.name);
            add_subscription(graph, publisher, slug, event, &to);
        }
    }
}

/// `calls`: an edge from the caller app to each declared function of the
/// callee, when the caller or the callee is wanted.
fn add_calls(
    graph: &mut Graph,
    by_slug: &HashMap<&str, &AppIn>,
    app: &AppIn,
    app_wanted: bool,
    wanted: &impl Fn(&AppIn) -> bool,
) {
    let Ok(declared) = DeclaredCalls::from_manifest(app.manifest.as_ref()) else {
        return;
    };
    let from = app_node_id(app.id);
    for (slug, function) in declared.pairs() {
        let callee = by_slug.get(slug).copied();
        if !app_wanted && !callee.is_some_and(wanted) {
            continue;
        }
        ensure_app(graph, app);
        let (to, problem) = match callee {
            None => (missing_app(graph, slug), Some("app_not_found")),
            Some(callee) => {
                let entry = callee.functions.iter().find(|f| f.name == function);
                let problem = match entry {
                    None => Some("function_not_found"),
                    Some(f) if !function_callable(Some(&f.spec)) => Some("not_callable"),
                    Some(_) => None,
                };
                ensure_app(graph, callee);
                (function_node(graph, callee, function), problem)
            }
        };
        let edge = graph.edge(&from, &to, EdgeKind::Calls, "calls");
        if let Some(problem) = problem {
            edge.problems.insert(problem.to_string());
        }
    }
}

/// One `on: "<slug>/<event>"` of the subscribing function `to`: an edge from
/// each publisher function that may publish the event, or from the publisher
/// app, marked, when none may.
fn add_subscription(
    graph: &mut Graph,
    publisher: Option<&AppIn>,
    slug: &str,
    event: &str,
    to: &str,
) {
    let Some(publisher) = publisher else {
        let from = missing_app(graph, slug);
        let edge = graph.edge(&from, to, EdgeKind::Publishes, "on");
        edge.events.insert(event.to_string());
        edge.problems.insert("app_not_found".into());
        return;
    };
    ensure_app(graph, publisher);
    let in_catalog = EventCatalog::from_manifest(publisher.manifest.as_ref())
        .is_ok_and(|c| c.events.contains_key(event));
    let publishing: Vec<&str> = publisher
        .functions
        .iter()
        .filter(|f| function_publishes(&f.spec).is_ok_and(|p| p.contains(event)))
        .map(|f| f.name.as_str())
        .collect();
    let mut froms: Vec<String> = publishing
        .iter()
        .map(|name| function_node(graph, publisher, name))
        .collect();
    if froms.is_empty() {
        froms.push(app_node_id(publisher.id));
    }
    for from in froms {
        let edge = graph.edge(&from, to, EdgeKind::Publishes, "events");
        edge.via.insert("on".into());
        edge.events.insert(event.to_string());
        if !in_catalog {
            edge.problems.insert("not_in_catalog".into());
        }
        if publishing.is_empty() {
            edge.problems.insert("never_published".into());
        }
    }
}

/// The app's node, for an app the workspace filter left out of the first pass.
fn ensure_app(graph: &mut Graph, app: &AppIn) {
    let node = graph.node(&app_node_id(app.id), NodeKind::App, &app.name);
    node.meta.entry("slug").or_insert_with(|| json!(app.slug));
    node.meta
        .entry("environment")
        .or_insert_with(|| json!(app.environment));
    node.meta
        .entry("workspace_id")
        .or_insert_with(|| json!(app.workspace_id));
}

/// `fn:<app>:<name>`, created under its app when new.
fn function_node(graph: &mut Graph, app: &AppIn, name: &str) -> String {
    let id = function_node_id(app.id, name);
    graph.node(&id, NodeKind::Function, name).parent = Some(app_node_id(app.id));
    id
}

/// `app:missing:<slug>`: a slug no app of the org holds.
fn missing_app(graph: &mut Graph, slug: &str) -> String {
    let id = format!("app:missing:{slug}");
    let node = graph.node(&id, NodeKind::App, slug);
    node.meta.insert("slug".into(), json!(slug));
    node.meta.insert("missing".into(), json!(true));
    id
}

#[cfg(test)]
#[path = "messaging_tests.rs"]
mod tests;

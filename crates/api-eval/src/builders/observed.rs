//! What an eval run did, read off its event stream: the facts a case's
//! `expect:` block is checked against ([`RunObserved`]). Fed only the
//! answer's events, not a memory reflection's.

use agentic_analytics::AnalyticsEvent;
use agentic_core::events::{CoreEvent, Event as AgenticEvent};
use oxy::exec_types::RunObserved;

use super::run_timing::source_name;

/// The FSM's `ChartRendered` also carries plain tables; those are not charts.
const TABLE_CHART_TYPE: &str = "table";

#[derive(Default)]
pub(super) struct Observer(RunObserved);

impl Observer {
    pub(super) fn observe(&mut self, ev: &AgenticEvent<AnalyticsEvent>) {
        match ev {
            AgenticEvent::Core(CoreEvent::ToolCall { name, .. }) => {
                self.0.tools.push(name.clone());
            }
            AgenticEvent::Domain(domain) => self.observe_domain(domain),
            AgenticEvent::Core(_) => {}
        }
    }

    fn observe_domain(&mut self, ev: &AnalyticsEvent) {
        match ev {
            AnalyticsEvent::QueryExecuted {
                success: true,
                source,
                is_preagg,
                ..
            } => {
                self.0.query_sources.push(source_name(*source).to_string());
                self.0.preagg |= *is_preagg;
            }
            AnalyticsEvent::ResultChart { .. } => self.0.charts += 1,
            AnalyticsEvent::ChartRendered { config, .. }
                if config.chart_type != TABLE_CHART_TYPE =>
            {
                self.0.charts += 1;
            }
            _ => {}
        }
    }

    /// The observations, with the answer's query source (worked out with the
    /// run's `final_sql`).
    pub(super) fn finish(self, answer_source: Option<String>) -> RunObserved {
        RunObserved {
            answer_source,
            ..self.0
        }
    }
}

/// The question a run stopped on, from the error text a headless run returns
/// when it suspends; `None` for any other failure.
pub(super) fn clarifying_question(error: &str) -> Option<&str> {
    error.strip_prefix(agentic_pipeline::CLARIFYING_QUESTION_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_analytics::QuerySource;

    fn executed(
        success: bool,
        source: QuerySource,
        is_preagg: bool,
    ) -> AgenticEvent<AnalyticsEvent> {
        AgenticEvent::Domain(AnalyticsEvent::QueryExecuted {
            query: "select 1".into(),
            row_count: 0,
            duration_ms: 0,
            success,
            error: None,
            columns: vec![],
            rows: vec![],
            source,
            is_preagg,
            sub_spec_index: None,
            semantic_query: None,
        })
    }

    fn tool_call(name: &str) -> AgenticEvent<AnalyticsEvent> {
        AgenticEvent::Core(CoreEvent::ToolCall {
            name: name.into(),
            input: "{}".into(),
            llm_duration_ms: 0,
            sub_spec_index: None,
        })
    }

    fn rendered(chart_type: &str) -> AgenticEvent<AnalyticsEvent> {
        let config: agentic_analytics::ChartConfig =
            serde_json::from_value(serde_json::json!({ "chart_type": chart_type })).unwrap();
        AgenticEvent::Domain(AnalyticsEvent::ChartRendered {
            config,
            columns: vec![],
            rows: vec![],
        })
    }

    fn observe_all(events: &[AgenticEvent<AnalyticsEvent>]) -> RunObserved {
        let mut observer = Observer::default();
        events.iter().for_each(|e| observer.observe(e));
        observer.finish(Some("semantic".into()))
    }

    #[test]
    fn records_tools_in_order_and_successful_query_sources() {
        let seen = observe_all(&[
            tool_call("search"),
            executed(false, QuerySource::Llm, true),
            tool_call("query"),
            executed(true, QuerySource::VerifiedSql, false),
            tool_call("query"),
        ]);
        assert_eq!(seen.tools, vec!["search", "query", "query"]);
        assert_eq!(seen.query_sources, vec!["verified_sql"]);
        assert!(!seen.preagg, "a failed query's is_preagg does not count");
        assert_eq!(seen.answer_source.as_deref(), Some("semantic"));
        assert_eq!(seen.asked_user, None);
    }

    #[test]
    fn a_successful_preagg_query_sets_preagg() {
        let seen = observe_all(&[
            executed(true, QuerySource::Semantic, true),
            executed(true, QuerySource::Llm, false),
        ]);
        assert!(seen.preagg);
    }

    fn react_chart() -> AgenticEvent<AnalyticsEvent> {
        AgenticEvent::Domain(AnalyticsEvent::ResultChart {
            result_id: "r1".into(),
            kind: agentic_analytics::react::ChartKind::Bar,
            title: None,
            x: "x".into(),
            series: None,
            y: vec!["y".into()],
            rows: vec![],
        })
    }

    #[test]
    fn counts_charts_but_not_fsm_tables() {
        let seen = observe_all(&[rendered("bar_chart"), rendered("table")]);
        assert_eq!(seen.charts, 1);
        assert_eq!(observe_all(&[react_chart()]).charts, 1);
    }

    #[test]
    fn reads_the_question_off_a_suspended_run_error() {
        let err = format!(
            "{}Which region?",
            agentic_pipeline::CLARIFYING_QUESTION_PREFIX
        );
        assert_eq!(clarifying_question(&err), Some("Which region?"));
        assert_eq!(clarifying_question("max iterations exceeded"), None);
    }
}

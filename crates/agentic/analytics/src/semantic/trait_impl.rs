//! `impl Catalog for SemanticCatalog` — the tool-facing catalog API.

use crate::catalog::{
    Catalog, CatalogError, ColumnRange, DimensionSummary, FreshnessTarget, JoinPath, MetricDef,
    MetricSummary, QueryContext, SampleTarget,
};
use crate::types::AnalyticsIntent;

use super::SemanticCatalog;

impl Catalog for SemanticCatalog {
    fn list_metrics(&self, query: &str) -> Vec<MetricSummary> {
        let q = query.to_lowercase();
        self.engine
            .views()
            .iter()
            .flat_map(|v| {
                let q2 = q.clone();
                let view_name = v.name.clone();
                v.measures_list()
                    .iter()
                    .filter(move |m| {
                        q2.is_empty()
                            || m.name.to_lowercase().contains(&q2)
                            || m.description
                                .as_deref()
                                .is_some_and(|d| d.to_lowercase().contains(&q2))
                            || view_name.to_lowercase().contains(&q2)
                    })
                    .map(move |m| MetricSummary {
                        // Return qualified view.measure name so downstream stages
                        // can use it directly in airlayer QueryRequests.
                        name: format!("{}.{}", v.name, m.name),
                        description: m.description.clone().unwrap_or_else(|| {
                            format!("{} measure from view `{}`", m.measure_type, v.name)
                        }),
                        metric_type: m.measure_type.to_string(),
                        expr: m.expr.clone(),
                    })
            })
            .collect()
    }

    fn list_dimensions(&self, metric: &str) -> Vec<DimensionSummary> {
        let Some((view, _)) = self.find_measure(metric) else {
            return vec![];
        };
        self.reachable_views(view)
            .into_iter()
            .flat_map(|v| {
                v.dimensions.iter().map(|d| DimensionSummary {
                    // Return qualified view.dimension name for airlayer compatibility.
                    name: format!("{}.{}", v.name, d.name),
                    description: d
                        .description
                        .clone()
                        .unwrap_or_else(|| format!("{} dimension", d.dimension_type)),
                    data_type: d.dimension_type.to_string(),
                })
            })
            .collect()
    }

    fn get_metric_definition(&self, metric: &str) -> Option<MetricDef> {
        let (view, measure) = self.find_measure(metric)?;
        Some(MetricDef {
            // Return qualified view.measure name for airlayer compatibility.
            name: format!("{}.{}", view.name, measure.name),
            expr: measure.expr.clone().unwrap_or_else(|| measure.name.clone()),
            metric_type: measure.measure_type.to_string(),
            table: view.table.clone().unwrap_or_else(|| view.name.clone()),
            description: measure.description.clone(),
            data_source: view.datasource.clone(),
        })
    }

    fn get_valid_dimensions(&self, metric: &str) -> Vec<DimensionSummary> {
        // Return only dimensions from the metric's own view — not joined views.
        let Some((view, _)) = self.find_measure(metric) else {
            return vec![];
        };
        view.dimensions
            .iter()
            .map(|d| DimensionSummary {
                // Return qualified view.dimension name for airlayer compatibility.
                name: format!("{}.{}", view.name, d.name),
                description: d
                    .description
                    .clone()
                    .unwrap_or_else(|| format!("{} dimension", d.dimension_type)),
                data_type: d.dimension_type.to_string(),
            })
            .collect()
    }

    fn get_column_range(&self, dimension: &str) -> Option<ColumnRange> {
        let (_, dim) = self.find_dimension(dimension)?;
        Some(ColumnRange {
            min: None,
            max: None,
            sample_values: dim
                .samples
                .as_ref()
                .map(|s| {
                    s.iter()
                        .map(|v| serde_json::Value::String(v.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            data_type: dim.dimension_type.to_string(),
        })
    }

    fn get_join_path(&self, from: &str, to: &str) -> Option<JoinPath> {
        let from_view = self.engine.view(from)?;
        let to_view = self.engine.view(to)?;

        let pk_keys: Vec<String> = from_view
            .entities
            .iter()
            .filter(|e| e.entity_type == oxy_airlayer_compat::schema::models::EntityType::Primary)
            .flat_map(|e| e.get_keys())
            .collect();

        let join_key = to_view.entities.iter().find(|e| {
            e.entity_type == oxy_airlayer_compat::schema::models::EntityType::Foreign
                && e.get_keys().iter().any(|k| pk_keys.contains(k))
        })?;

        let key = join_key.get_keys().into_iter().next()?;
        let from_table = from_view.table.as_deref().unwrap_or(&from_view.name);
        let to_table = to_view.table.as_deref().unwrap_or(&to_view.name);

        Some(JoinPath {
            path: format!("{from_table} JOIN {to_table} ON {from_table}.{key} = {to_table}.{key}"),
            join_type: "INNER".to_string(),
        })
    }

    fn resolve_sample_target(&self, table: &str, column: &str) -> Option<SampleTarget> {
        // Find the view matching `table` (by view name or underlying table name).
        let view = self.engine.views().iter().find(|v| {
            v.name.eq_ignore_ascii_case(table)
                || v.table
                    .as_deref()
                    .is_some_and(|t| t.eq_ignore_ascii_case(table))
        })?;

        // Look up `column` as a dimension first, then as a measure.
        let underlying_table = view.table.clone().unwrap_or_else(|| view.name.clone());

        if let Some(dim) = view
            .dimensions
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case(column))
        {
            return Some(SampleTarget {
                table: underlying_table,
                column_expr: dim.expr.clone(),
                static_samples: dim.samples.clone().unwrap_or_default(),
                data_type: Some(dim.dimension_type.to_string()),
                datasource: view.datasource.clone(),
            });
        }

        if let Some(measure) = view
            .measures_list()
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(column))
        {
            return Some(SampleTarget {
                table: underlying_table,
                column_expr: measure.expr.clone().unwrap_or_else(|| measure.name.clone()),
                static_samples: measure.samples.clone().unwrap_or_default(),
                data_type: None,
                datasource: view.datasource.clone(),
            });
        }

        None
    }

    fn resolve_freshness_target(&self, view: &str) -> Option<FreshnessTarget> {
        let v = self.engine.views().iter().find(|x| {
            x.name.eq_ignore_ascii_case(view)
                || x.table
                    .as_deref()
                    .is_some_and(|t| t.eq_ignore_ascii_case(view))
        })?;

        let meta_first = |key: &str| -> Option<String> {
            v.meta
                .as_ref()
                .and_then(|m| m.get(key))
                .and_then(|values| values.first())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };

        // Watermark resolution: a `meta:`-declared column wins (using the
        // matching dimension's expr when one exists, else the raw column
        // name); otherwise fall back to the first date/datetime dimension.
        let (watermark_name, watermark_expr, declared) =
            if let Some(col) = meta_first("freshness_watermark_column") {
                match v.dimensions.iter().find(|d| {
                    d.name.eq_ignore_ascii_case(&col) || d.expr.eq_ignore_ascii_case(&col)
                }) {
                    Some(d) => (d.name.clone(), d.expr.clone(), true),
                    None => (col.clone(), col, true),
                }
            } else {
                let d = v.dimensions.iter().find(|d| {
                    matches!(
                        d.dimension_type,
                        oxy_airlayer_compat::schema::models::DimensionType::Date
                            | oxy_airlayer_compat::schema::models::DimensionType::Datetime
                    )
                })?;
                (d.name.clone(), d.expr.clone(), false)
            };

        let refresh_key_sql = match &v.refresh_key {
            Some(oxy_airlayer_compat::schema::models::RefreshKey::Sql(sql)) => Some(sql.clone()),
            _ => None,
        };

        Some(FreshnessTarget {
            view: v.name.clone(),
            table: v.table.clone().unwrap_or_else(|| v.name.clone()),
            datasource: v.datasource.clone(),
            watermark_expr,
            watermark_name,
            declared,
            expected_cadence: meta_first("freshness_expected_cadence"),
            complete_through: meta_first("freshness_complete_through"),
            caveat: meta_first("freshness_caveat"),
            refresh_key_sql,
        })
    }

    fn try_compile(&self, intent: &AnalyticsIntent) -> Result<String, CatalogError> {
        if intent.metrics.is_empty() {
            return Err(CatalogError::TooComplex("no metrics in intent".into()));
        }
        if Self::filters_are_complex(&intent.filters) {
            return Err(CatalogError::TooComplex(
                "filters contain SQL functions or complex expressions".into(),
            ));
        }

        // Qualify metrics first (no view preference).
        let measures = self
            .qualify_names(&intent.metrics, true, &[])
            .ok_or_else(|| {
                let bad = intent
                    .metrics
                    .iter()
                    .find(|m| self.find_measure(m).is_none())
                    .cloned()
                    .unwrap_or_default();
                tracing::info!(
                    metrics = ?intent.metrics,
                    unresolvable = %bad,
                    "qualify_names failed"
                );
                CatalogError::UnresolvableMetric(bad)
            })?;

        // Extract view names from qualified metrics to use as dimension hints.
        // e.g. ["cardio.max_heart_rate"] → ["cardio"]
        let metric_views: Vec<String> = measures
            .iter()
            .filter_map(|m| m.split('.').next().map(String::from))
            .collect();

        // Qualify dimensions, preferring the metrics' views so that shared
        // names like "date" resolve to the same view as the measures.
        let dimensions = self
            .qualify_names(&intent.dimensions, false, &metric_views)
            .ok_or_else(|| {
                let bad = intent
                    .dimensions
                    .iter()
                    .find(|d| self.find_dimension(d).is_none())
                    .cloned()
                    .unwrap_or_default();
                tracing::info!(
                    dimensions = ?intent.dimensions,
                    unresolvable = %bad,
                    "qualify_names failed"
                );
                CatalogError::UnresolvableDimension(bad)
            })?;

        tracing::debug!(?measures, ?dimensions, "qualified");

        // Parse intent filters into structured airlayer QueryFilters.
        // Filters that contain SQL functions or complex expressions cannot be
        // represented in airlayer's filter API, so we bail to TooComplex.
        let filters = self.parse_intent_filters(&intent.filters, &metric_views)?;
        tracing::debug!(?filters, "parsed filters");

        let request = oxy_airlayer_compat::engine::query::QueryRequest {
            measures,
            dimensions,
            filters,
            segments: vec![],
            time_dimensions: vec![],
            order: vec![],
            limit: None,
            offset: None,
            timezone: None,
            ungrouped: false,
            through: vec![],
            motif: None,
            motif_params: Default::default(),
        };

        let result = self.engine.compile_query(&request).map_err(|e| {
            tracing::debug!(error = %e, "airlayer compile_query failed");
            CatalogError::TooComplex(format!("airlayer compile error: {e}"))
        })?;

        let sql = crate::airlayer_compat::substitute_params(
            &crate::airlayer_compat::request_dialect(&self.engine, &request),
            &result.sql,
            &result.params,
        );

        Ok(sql)
    }

    fn get_context(&self, intent: &AnalyticsIntent) -> QueryContext {
        let metric_defs = intent
            .metrics
            .iter()
            .filter_map(|m| self.get_metric_definition(m))
            .collect();

        let dim_defs = intent
            .dimensions
            .iter()
            .filter_map(|d| self.find_dimension(d))
            .map(|(_, d)| DimensionSummary {
                name: d.name.clone(),
                description: d.description.clone().unwrap_or_default(),
                data_type: d.dimension_type.to_string(),
            })
            .collect();

        // Collect join paths relevant to the intent's metrics.
        let mut join_paths = Vec::new();
        for metric in &intent.metrics {
            if let Some((view, _)) = self.find_measure(metric) {
                for other in self.engine.views() {
                    if other.name == view.name {
                        continue;
                    }
                    if let Some(jp) = self.get_join_path(&view.name, &other.name) {
                        let key = (view.name.clone(), other.name.clone());
                        if !join_paths
                            .iter()
                            .any(|(a, b, _): &(String, String, JoinPath)| {
                                (*a == key.0 && *b == key.1) || (*a == key.1 && *b == key.0)
                            })
                        {
                            join_paths.push((key.0, key.1, jp));
                        }
                    }
                }
            }
        }

        // Build a prompt-ready description of all views.
        let schema_lines: Vec<String> = self
            .engine
            .views()
            .iter()
            .map(|v| {
                let source = v
                    .table
                    .as_deref()
                    .unwrap_or_else(|| v.sql.as_deref().unwrap_or("(sql)"));
                let measures: Vec<String> = v
                    .measures_list()
                    .iter()
                    .map(|m| {
                        let expr = m.expr.as_deref().unwrap_or(&m.name);
                        format!("{}({}) AS {}", m.measure_type, expr, m.name)
                    })
                    .collect();
                let dims: Vec<String> = v
                    .dimensions
                    .iter()
                    .map(|d| format!("{}:{}", d.name, d.dimension_type))
                    .collect();
                format!(
                    "view `{}` (source: {})  measures=[{}]  dimensions=[{}]",
                    v.name,
                    source,
                    measures.join(", "),
                    dims.join(", ")
                )
            })
            .collect();

        QueryContext {
            metric_definitions: metric_defs,
            dimension_definitions: dim_defs,
            join_paths,
            schema_description: schema_lines.join("\n"),
            compile_failure_reason: None,
        }
    }

    fn table_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .engine
            .views()
            .iter()
            .map(|v| v.table.clone().unwrap_or_else(|| v.name.clone()))
            .collect();
        names.sort();
        names.dedup();
        names
    }

    fn connector_for_table(&self, table: &str) -> Option<&str> {
        self.engine.views().iter().find_map(|v| {
            let matches = v.name.eq_ignore_ascii_case(table)
                || v.table
                    .as_deref()
                    .is_some_and(|t| t.eq_ignore_ascii_case(table));
            if matches {
                v.datasource.as_deref()
            } else {
                None
            }
        })
    }
}

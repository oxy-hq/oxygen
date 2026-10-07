export const VALIDATION_STAGES = [
  {
    stage: "specified",
    label: "After Specify",
    description: "Checks the query spec before any SQL is written."
  },
  {
    stage: "solvable",
    label: "After Solve",
    description: "Checks the generated SQL before it runs."
  },
  {
    stage: "solved",
    label: "After Execute",
    description: "Checks the result before it is interpreted."
  }
] as const;

export type ValidationStage = (typeof VALIDATION_STAGES)[number]["stage"];

interface RuleOption {
  value: string;
  label: string;
  /** Registered, but not in the set that runs when `validation:` is absent. */
  offByDefault?: true;
}

/**
 * Every rule the backend registers, under the one stage it is registered for
 * (`crates/agentic/analytics/src/validation/registry.rs`). A rule listed under
 * another stage fails the agent build, so each stage offers only its own.
 *
 * Without `offByDefault`, a rule is in `ValidationConfig::default_all_rules()`
 * (`validation/config.rs`), and listed here in that order.
 */
export const RULES_BY_STAGE: Record<ValidationStage, readonly RuleOption[]> = {
  specified: [
    { value: "metric_resolves", label: "Metric Resolves" },
    { value: "join_key_exists", label: "Join Key Exists" },
    { value: "filter_unambiguous", label: "Filter Unambiguous" }
  ],
  solvable: [
    { value: "sql_syntax", label: "SQL Syntax" },
    { value: "tables_exist_in_catalog", label: "Tables Exist in Catalog" },
    { value: "spec_tables_present", label: "Spec Tables Present" },
    { value: "column_refs_valid", label: "Column Refs Valid" },
    { value: "timeseries_order_by_check", label: "Time Series Order By" }
  ],
  solved: [
    { value: "non_empty", label: "Non Empty" },
    { value: "truncation_warning", label: "Truncation Warning" },
    { value: "no_nan_inf", label: "No NaN/Inf" },
    { value: "outlier_detection", label: "Outlier Detection" },
    { value: "null_ratio_check", label: "Null Ratio" },
    { value: "duplicate_row_check", label: "Duplicate Rows" },
    { value: "freshness_check", label: "Freshness" },
    { value: "shape_match", label: "Shape Match", offByDefault: true },
    { value: "timeseries_date_check", label: "Time Series Dates", offByDefault: true }
  ]
};

type NumericRuleParam =
  | "threshold_sigma"
  | "min_rows"
  | "threshold"
  | "max_duplicate_ratio"
  | "threshold_days";

interface RuleParamField {
  key: NumericRuleParam;
  label: string;
  /** The backend's default, shown while the field is blank. */
  placeholder: string;
  step: number;
  min: number;
  max?: number;
}

/**
 * The parameters each rule reads (`validation/config.rs` and the rule
 * constructors). `sql_syntax` also accepts `dialect`, but the rule parses every
 * dialect as generic SQL, so it has no control here; a value in the file is kept.
 */
export const RULE_PARAM_FIELDS: Readonly<Record<string, readonly RuleParamField[]>> = {
  outlier_detection: [
    {
      key: "threshold_sigma",
      label: "Threshold (σ)",
      placeholder: "Default: 5.0",
      step: 0.1,
      min: 0
    },
    { key: "min_rows", label: "Min Rows", placeholder: "Default: 4", step: 1, min: 0 }
  ],
  null_ratio_check: [
    {
      key: "threshold",
      label: "Max Null Ratio",
      placeholder: "Default: 0.5",
      step: 0.05,
      min: 0,
      max: 1
    }
  ],
  duplicate_row_check: [
    {
      key: "max_duplicate_ratio",
      label: "Max Duplicate Ratio",
      placeholder: "Default: 0.1",
      step: 0.05,
      min: 0,
      max: 1
    }
  ],
  freshness_check: [
    { key: "threshold_days", label: "Threshold (days)", placeholder: "Default: 1", step: 1, min: 0 }
  ]
};

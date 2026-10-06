use super::*;
use chrono::TimeZone;

const URL: &str = "https://app.example.com/acme/workspaces/w/ide/semantic?view=anomalies";

fn day(d: u32) -> DateTime<FixedOffset> {
    Utc.with_ymd_and_hms(2026, 10, d, 0, 0, 0)
        .unwrap()
        .fixed_offset()
}

/// A high daily bucket on 2026-10-04, 1204 against an expected 1571.
fn bucket(event: u128) -> Bucket {
    Bucket {
        event_id: Uuid::from_u128(event),
        measure: "sales.net".into(),
        label: Some("Net sales".into()),
        granularity: "day".into(),
        period_start: day(4),
        observed: 1204.0,
        expected: 1571.0,
        z_score: -4.0,
        severity: "high".into(),
        dimension_key: String::new(),
        cohort_id: None,
        cohort_label: None,
    }
}

fn body(message: &SlackMessage) -> String {
    message.blocks[1]["text"]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

fn footer(message: &SlackMessage) -> Option<String> {
    message.blocks[2]["elements"][0]["text"]
        .as_str()
        .map(str::to_string)
}

#[test]
fn nothing_claimed_is_no_message() {
    assert_eq!(build("Acme", Some(URL), &[]), None);
}

#[test]
fn one_event_reads_as_one_sentence() {
    let message = build("Acme", Some(URL), &[bucket(1)]).unwrap();
    assert_eq!(message.text, "1 new insight in Acme");
    assert_eq!(message.events, 1);
    assert_eq!(
        message.blocks[0]["text"]["text"],
        "*1 new insight* in *Acme*"
    );
    assert_eq!(
        body(&message),
        "🔴 *Net sales* — 23.4% below expected on 2026-10-04 (1,204 vs 1,571)"
    );
    assert_eq!(
        footer(&message).unwrap(),
        format!("<{URL}|Open the Insights Inbox>")
    );
}

#[test]
fn a_segment_is_named_without_its_view() {
    let b = Bucket {
        dimension_key: "sales_daily.restaurant_id=loc-abc;sales_daily.channel=web".into(),
        ..bucket(1)
    };
    assert!(
        body(&build("Acme", None, &[b]).unwrap())
            .starts_with("🔴 *Net sales* · restaurant_id=loc-abc, channel=web — "),
    );
}

#[test]
fn an_unlabelled_monitor_falls_back_to_its_measure() {
    let b = Bucket {
        label: None,
        ..bucket(1)
    };
    assert!(body(&build("Acme", None, &[b]).unwrap()).starts_with("🔴 *sales.net* — "));
}

/// The row the line links to shows the peak bucket's numbers and the event's
/// max severity, so the line has to roll up the same way: the peak is the
/// largest `|z|` (earliest on a tie), and a `low` continuation bucket with the
/// larger `|z|` must not badge the event `low`.
#[test]
fn a_multi_bucket_event_reports_its_peak_and_its_worst_severity() {
    let breach = Bucket {
        period_start: day(2),
        z_score: -3.0,
        ..bucket(1)
    };
    let slide = Bucket {
        period_start: day(3),
        z_score: -5.0,
        observed: 900.0,
        expected: 1500.0,
        severity: "low".into(),
        ..bucket(1)
    };
    let tie = Bucket {
        period_start: day(4),
        z_score: 5.0,
        ..bucket(1)
    };
    let message = build("Acme", None, &[tie, slide, breach]).unwrap();
    assert_eq!(message.events, 1);
    assert_eq!(
        body(&message),
        "🔴 *Net sales* — 40.0% below expected on 2026-10-03 (900 vs 1,500) \
         · 3 days flagged since 2026-10-02"
    );
}

#[test]
fn week_and_month_buckets_are_named_by_their_grain() {
    let week = Bucket {
        granularity: "week".into(),
        period_start: day(5),
        ..bucket(1)
    };
    let month = Bucket {
        granularity: "month".into(),
        period_start: day(1),
        observed: 2000.0,
        ..bucket(2)
    };
    let text = body(&build("Acme", None, &[week, month]).unwrap());
    assert!(
        text.contains("below expected in the week of 2026-10-05 ("),
        "{text}"
    );
    assert!(text.contains("above expected in Oct 2026 ("), "{text}");
}

/// A breach past an expectation of zero has no ratio to print. The Insights
/// Inbox shows a dash there; the line says the direction and the two values.
#[test]
fn an_expectation_of_zero_prints_no_percentage() {
    let b = Bucket {
        observed: 12.0,
        expected: 0.0,
        ..bucket(1)
    };
    assert_eq!(
        body(&build("Acme", None, &[b]).unwrap()),
        "🔴 *Net sales* — above expected on 2026-10-04 (12 vs 0)"
    );
}

#[test]
fn lines_run_most_severe_first_then_furthest_from_expectation() {
    let mild = Bucket {
        severity: "medium".into(),
        observed: 100.0,
        ..bucket(1)
    };
    let far = Bucket {
        label: Some("Far".into()),
        observed: 500.0,
        ..bucket(2)
    };
    let near = Bucket {
        label: Some("Near".into()),
        observed: 1500.0,
        ..bucket(3)
    };
    let text = body(&build("Acme", None, &[mild, near, far]).unwrap());
    let heads: Vec<&str> = text
        .lines()
        .map(|l| l.split(" — ").next().unwrap())
        .collect();
    assert_eq!(heads, ["🔴 *Far*", "🔴 *Near*", "🟠 *Net sales*"]);
}

/// A chain-wide move is one cohort across many segments. It is one line with a
/// count, carrying the tenant calendar's name for the day when there is one —
/// and the header still counts every event, because that is what the inbox
/// will show.
#[test]
fn events_that_fired_as_a_cohort_share_a_line() {
    let cohort = Uuid::from_u128(77);
    let member = |n: u128| Bucket {
        dimension_key: format!("sales_daily.restaurant_id=loc-{n}"),
        cohort_id: Some(cohort),
        cohort_label: Some("Independence Day".into()),
        ..bucket(n)
    };
    let lone = Bucket {
        label: Some("Labor".into()),
        severity: "medium".into(),
        ..bucket(9)
    };
    let message = build("Acme", None, &[member(1), member(2), member(3), lone]).unwrap();
    assert_eq!(message.events, 4);
    assert_eq!(message.text, "4 new insights in Acme");
    let text = body(&message);
    assert_eq!(text.lines().count(), 2, "{text}");
    assert_eq!(
        text.lines().next().unwrap(),
        "🔴 *Net sales* — below expected across 3 segments on 2026-10-04 · Independence Day"
    );
    assert_eq!(
        footer(&message),
        None,
        "nothing hidden and no link: no footer"
    );
}

/// A cohort of one is just an event: the other members were below the
/// threshold or already announced, so "across 1 segments" would be noise.
#[test]
fn a_lone_cohort_member_is_an_ordinary_line() {
    let b = Bucket {
        dimension_key: "sales_daily.restaurant_id=loc-1".into(),
        cohort_id: Some(Uuid::from_u128(77)),
        cohort_label: Some("Independence Day".into()),
        ..bucket(1)
    };
    assert_eq!(
        body(&build("Acme", None, &[b]).unwrap()),
        "🔴 *Net sales* · restaurant_id=loc-1 — 23.4% below expected on 2026-10-04 \
         (1,204 vs 1,571) · Independence Day"
    );
}

#[test]
fn a_long_list_is_capped_and_says_how_many_it_left_out() {
    let buckets: Vec<Bucket> = (1..=14).map(bucket).collect();
    let message = build("Acme", Some(URL), &buckets).unwrap();
    assert_eq!(message.events, 14);
    assert_eq!(body(&message).lines().count(), MAX_LINES);
    assert_eq!(
        footer(&message).unwrap(),
        format!("…and 4 more · <{URL}|Open the Insights Inbox>")
    );
}

/// "More" counts events, and a cohort line already stands for several.
#[test]
fn a_cohort_behind_the_cap_is_counted_by_its_members() {
    let cohort = Uuid::from_u128(77);
    let mut buckets: Vec<Bucket> = (1..=MAX_LINES as u128).map(bucket).collect();
    buckets.extend((100..103).map(|n| Bucket {
        severity: "low".into(),
        cohort_id: Some(cohort),
        ..bucket(n)
    }));
    let message = build("Acme", None, &buckets).unwrap();
    assert_eq!(message.events, MAX_LINES + 3);
    assert_eq!(footer(&message).unwrap(), "…and 3 more");
}

/// Labels, segments and the workspace name are tenant text. Unescaped, a
/// dimension value of `<!channel>` pages everyone in the channel.
#[test]
fn tenant_text_cannot_inject_slack_markup() {
    let b = Bucket {
        label: Some("R&D <spend>".into()),
        dimension_key: "t.team=<!channel>".into(),
        cohort_label: Some("<@U123>".into()),
        cohort_id: Some(Uuid::from_u128(5)),
        ..bucket(1)
    };
    let message = build("A <b> & C", None, &[b]).unwrap();
    let all = format!("{}{}", message.blocks, message.text);
    assert!(
        !all.contains("<!channel>") && !all.contains("<@U123>"),
        "{all}"
    );
    assert_eq!(message.text, "1 new insight in A &lt;b&gt; &amp; C");
    let text = body(&message);
    assert!(
        text.contains("*R&amp;D &lt;spend&gt;* · team=&lt;!channel&gt; — "),
        "{text}"
    );
    assert!(text.ends_with(" · &lt;@U123&gt;"), "{text}");
}

/// Slack refuses a section past 3000 characters, and one refused post would
/// lose the whole announcement — so lines give way before the limit does.
#[test]
fn the_list_stays_inside_one_slack_section() {
    let long = "x".repeat(400);
    let buckets: Vec<Bucket> = (1..=MAX_LINES as u128)
        .map(|n| Bucket {
            label: Some(long.clone()),
            dimension_key: format!("t.{long}={long}"),
            cohort_label: Some(long.clone()),
            cohort_id: Some(Uuid::from_u128(1000 + n)),
            ..bucket(n)
        })
        .collect();
    let message = build("Acme", None, &buckets).unwrap();
    let text = body(&message);
    assert!(text.chars().count() <= 3000, "{}", text.chars().count());
    let shown = text.lines().count();
    assert!(shown >= 1 && shown < MAX_LINES, "{shown}");
    assert_eq!(
        footer(&message).unwrap(),
        format!("…and {} more", MAX_LINES - shown)
    );
}

#[test]
fn values_are_grouped_above_a_thousand_and_trimmed_below() {
    for (value, shown) in [
        (1_234_567.4, "1,234,567"),
        (-9_876.5, "-9,877"),
        (999.6, "1,000"),
        (123.4, "123"),
        (12.5, "12.5"),
        (3.0, "3"),
        (0.0345, "0.0345"),
        (-0.00004, "0"),
        (f64::NAN, "n/a"),
    ] {
        assert_eq!(format_value(value), shown, "{value}");
    }
}

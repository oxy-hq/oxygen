# BIRD Mini-Dev FSM baseline

Eval model: `claude-opus-4-8` ($5.0/M input, $25.0/M output, $0.5/M cache read, $6.25/M cache write). Judge model: `claude-opus-4-8`.

**Answer accuracy: 44.4% (72/162 judged)**; EX 11.7%, Soft-F1 25.5%; 162 graded, 1 skipped, 0 judge errors; $118.09 agent cost.

Context: 53 of 72 judge-CORRECT answers had EX FAIL (how much strict result-set EX under-reads the FSM).

Latency and LLM rounds average the measured runs only (n per row); run failures carry no duration, rounds or tokens.

| db | answer acc (correct/judged) | judge errors | EX | Soft-F1 | graded | skipped | measured n | latency s mean / median | LLM rounds | tokens uncached in / cache read / cache write / out | $ |
|---|---|---|---|---|---|---|---|---|---|---|---|
| financial | 39.1% (18/46) | 0 | 10.9% | 21.6% | 46 | 0 | 41 | 84.1 / 41.3 | 12.8 | 1,545,414 / 4,282,726 / 1,510,258 / 223,607 | $24.90 |
| debit_card_specializing | 15.4% (2/13) | 0 | 0.0% | 7.7% | 13 | 1 | 13 | 35.4 / 36.1 | 6.5 | 170,309 / 578,227 / 188,311 / 30,242 | $3.07 |
| california_schools | 31.2% (15/48) | 0 | 10.4% | 21.8% | 48 | 0 | 42 | 189.0 / 38.3 | 38.9 | 3,604,487 / 14,093,043 / 2,888,659 / 501,286 | $55.66 |
| formula_1 | 67.3% (37/55) | 0 | 16.4% | 36.2% | 55 | 0 | 54 | 63.7 / 34.2 | 11.5 | 2,149,336 / 6,413,830 / 2,420,732 / 215,097 | $34.46 |
| **total** | 44.4% (72/162) | 0 | 11.7% | 25.5% | 162 | 1 | 150 | 101.9 / 36.6 | 19.1 | 7,469,546 / 25,367,826 / 7,007,960 / 970,232 | $118.09 |

## Skipped (gold error)

- debit_card_specializing 149: gold sql failed: query failed: [42702] column reference "round" is ambiguous SQL: WITH annual AS (SELECT c.segment, c.customerid, SUM(CAST(y.consumption AS DOUBLE PRECISION)) AS total FROM customers A...

## Failures (judge INCORRECT)

- financial 194: The answer gives account ID 2,679 but the gold account_id is 3214; the salary difference 4431 matches but the account ID does not.
- financial 197: The answer states 0% increase, but the gold result is approximately 430.45%.
- financial 198: The answer only shows 15 of 364 rows and omits the remaining account IDs and their frequencies required by the list.
- financial 204: The answer states 30,996 but the gold count is 19,143.
- financial 205: The answer's values and the identified highest district (Ostrava) disagree with the gold, where Brno - mesto has the highest average.
- financial 211: Answer gives 11,400 and 4,341, but gold values are 1,556 and 337.
- financial 212: The answer explicitly omits the counterparty_count column, which the question required for every account.
- financial 214: The answer declines to provide the ranking and never states the gold pair (AB, 44119823, 12 payments, 108408).
- financial 215: Answer gives 93.44% but gold is 93.31%, which differ at two-decimal precision.
- financial 216: The answer omits the most-populous-district names and its salary/unemployment averages don't match the gold rows.
- financial 217: Overall average (4850.15 vs 5030.83) and difference (+65.52 vs -115.16) are wrong.
- financial 219: The answer omits all average balance figures (per-district, region-wide, and highest-average district Liberec) required by the question.
- financial 220: Average salary given (8,921.80) does not match gold (8985.75).
- financial 222: Answer says home district is "Yes" but gold shows "No", and omits several required regional values.
- financial 223: The answer explicitly omits the per-district average-of-averages metric (23639.03 and 19809.53), which the question required.
- financial 224: The answer lists ten identical Strakonice/192552 rows, but gold shows ten distinct loans with varying districts, amounts, dates, and balances.
- financial 225: The gold has 21 clients but the answer only names one (client 3687), missing all others.
- financial 227: The answer claims no counterparties exist, but the gold rows list three counterparties with payment counts and totals.
- financial 229: The answer states 1,634 CZK, but the gold net interest is -505 CZK.
- financial 231: Answer gives Prague with wrong district count, population, and accounts; gold is south Moravia.
- financial 235: The answer's client counts, payment counts, and totals all disagree with the gold rows (e.g. Prague 72 vs 96, 511,841.72 vs 14,125,240).
- financial 236: The answer states 9,061.89 but the gold value is 9045.64, which do not match.
- financial 239: Young 9,085 vs gold 9047 and old 9,163 vs gold 9038 both differ beyond rounding.
- financial 213: Run failed with error: agentic pipeline asked a clarifying question: This request needs a nested, multi-grain computation that the structured query builder can't express faithfully in one pass. Specif...
- financial 221: Run failed with error: agentic pipeline asked a clarifying question: The data distinguishes two kinds of "withdrawal": VYDAJ (all debit / outgoing / money-leaving transactions, the general "amount wit...
- financial 230: Run failed with error: agentic pipeline asked a clarifying question: This profiling request needs several things the structured query builder can't express faithfully: (a) "top-3 loan amounts includin...
- financial 233: Run failed with error: agentic pipeline asked a clarifying question: This request needs a few things the semantic layer can't express directly — most importantly a *distinct count of external counterp...
- financial 234: Run failed with error: agentic pipeline asked a clarifying question: This profile needs per-account logic that the structured query builder can't express in a single query — specifically: (a) two diff...
- debit_card_specializing 151: The answer gives 0.0000 but the gold value is 5.8152.
- debit_card_specializing 152: The answer states 1,869 customers but the gold result is 1517.
- debit_card_specializing 153: The answer fails to provide the per-segment counts (1123, 2031, 21869) and misidentifies the segments and totals.
- debit_card_specializing 154: All three segment counts differ from gold (KAM 1836 vs 1833, LAM 3541 vs 3539, SME 22274 vs 22246).
- debit_card_specializing 155: The gold answer is 3670 minutes, but the response gives 56 and fails to provide the elapsed minutes.
- debit_card_specializing 156: Answer gives CZK 25,978 and EUR 1,673, but gold rows are CZK 61 and EUR 4.
- debit_card_specializing 157: EUR avg 3627.02 vs gold 3627.03 and CZK avg 44462.37 vs gold 44462.45 both differ at two decimals.
- debit_card_specializing 158: The answer gives 338,745,952 but the gold difference is 338,745,665.
- debit_card_specializing 159: The answer gives 3.01, but the gold value is 6.02.
- debit_card_specializing 161: The answer states 1,773 customers, but the gold count is 335.
- debit_card_specializing 162: The answer states 1529 but the gold count is 1518.
- california_schools 0: The gold answer is 0.9049, but the answer claims there are no matching schools (NULL).
- california_schools 1: The answer lists only 10 schools (claiming 359 total vs gold's 57) and most named schools don't match the gold rows.
- california_schools 2: The gold has five rates (0.917948, 0.999329, 0.896499, 0.89582, 0.91418) but the answer only provides two and claims three are unavailable.
- california_schools 4: The answer gives "25 Churchill Avenue" and NULL name, but the gold is "5172 Orange Avenue" and "Oxford Academy".
- california_schools 8: The answer lists only one email but the gold rows contain two distinct emails, missing j.hernandez@realjourney.org.
- california_schools 10: The gold average is 41.33, but the answer claims no schools matched and the result is undefined.
- california_schools 13: The gold row shows the city is Hayward, but the answer claims no matching school was found.
- california_schools 19: The gold answer is grade level "P" but the answer claims no schools matched.
- california_schools 20: The two names are Avetik and Stacy (Avetik runs 4 at Madera COE, Stacy 8 at Yosemite Unified); the answer named Gary instead of Avetik.
- california_schools 21: The answer gives 58.25, but the gold value is 5.42.
- california_schools 22: The answer failed to return the school; gold shows "Oakland Technical High" with level "High School".
- california_schools 24: The answer fails to identify Butte Valley Middle with latitude 41.96 and its school type, incorrectly claiming no usable data exists.
- california_schools 25: The answer gives 685.00 but the gold value is 14.57.
- california_schools 26: School counts, enrollment, FRPM averages, and SAT averages all differ from the gold rows.
- california_schools 30: Gold runner-up is FAME Public Charter (505), but the answer names a different school with score 515.
- california_schools 31: The answer states 854, but the gold value is 952.
- california_schools 32: The answer reports Kings County with wrong values, while the gold shows Alameda with 13 schools and different figures.
- california_schools 35: The answer claims no school matches, but the gold rows show Skyline High in Oakland with full enrollment, FRPM, and SAT details.
- california_schools 37: The answer claims no school was found, but the gold rows show a complete profile for Dougherty Valley High with all requested values.
- california_schools 38: San Joaquin has 10 schools (not 5) with top school California Connections Academy @ Ripon, which the answer gets wrong.
- california_schools 41: The answer gives a wrong count (7,271) and a sample list that doesn't match the gold's 121 specific schools.
- california_schools 42: The answer only lists 15 of the 929 gold schools and gives the wrong total count (1,794), failing to name all required schools.
- california_schools 43: Multiple counts differ from gold (e.g., Los Angeles 309 vs 305, San Diego 88 vs 86, San Bernardino 64 vs 60, Orange/San Bernardino order wrong).
- california_schools 44: The answer gives "Laurie Blue" but the gold administrator is "Zachary Larsen".
- california_schools 46: The answer omits most of the 31 cities (only lists 10), failing to name all entities the gold rows list.
- california_schools 47: Answer claims 175 schools total but gold has 182 rows, and only a partial preview is given.
- california_schools 48: The answer omits four counties with 0 test takers (Humboldt, Sacramento, San Bernardino, Ventura) and wrongly claims only three counties qualify.
- california_schools 11: Run failed with error: agentic pipeline asked a clarifying question: The agent used all 5 allotted tool rounds. Continue with more rounds?
- california_schools 14: Run failed with error: max iterations exceeded
- california_schools 15: Run failed with error: max iterations exceeded
- california_schools 16: Run failed with error: fatal: SyntaxError { query: "SELECT\n AVG(\"satscores\".\"avgscrmath\") AS \"satscores__avg_math_score\"\nFROM\n satscores AS \"satscores\"\nLEFT JOIN schools AS \"schools\" ON ...
- california_schools 17: Run failed with error: agentic pipeline asked a clarifying question: The agent used all 5 allotted tool rounds. Continue with more rounds?
- california_schools 40: Run failed with error: agentic pipeline asked a clarifying question: To define "regular, non-charter schools," how should I interpret "regular"? The CA datasets have no literal "Regular" type, so the ...
- formula_1 243: The answer states 1.17% but the gold value is 1.16%, which differ at two-decimal precision.
- formula_1 246: The answer gives a count (1,323) instead of the required percentage (64.88%).
- formula_1 251: The answer fails to provide the average (39.71), stating it cannot be computed.
- formula_1 252: Missing Markus Winkelhock and André Lotterer (both with 0 Q3 times).
- formula_1 256: The answer states 1 driver, but the gold value is 9.
- formula_1 258: Gold shows Mike=3 and Juan=2 only, but answer gives Mike=4, Juan=3, plus many extra names.
- formula_1 264: The answer gives 0.42, but the gold average is 29.18.
- formula_1 265: The answer states 39, but the gold count is 23.
- formula_1 266: The answer gives Manzon 16 points, but the gold shows manzon with 0.
- formula_1 267: The answer reports no results, but the gold rows contain many editions and two drivers (Anthony Davidson, Jarno Trulli).
- formula_1 272: Ferrari's points (5781.5 vs gold 6991.5) and Toro Rosso's (263 vs gold 381) don't match the gold values.
- formula_1 275: The question explicitly requires every nationality including those with count 0, but the answer omits all 16 zero-count nationalities.
- formula_1 276: The pit stop totals (77, 81, 82) don't match the gold values (1069, 905, 898) for 2011–2013.
- formula_1 279: Races started is 7 in gold, but the answer states 37.
- formula_1 290: The answer fails to name the latest circuit (gold: americas), leaving it blank/unidentified.
- formula_1 291: The answer lists André Lotterer as third, but the gold third driver is Jenson Button.
- formula_1 293: Answer states 5 drivers, but gold rows show 4.
- formula_1 244: Run failed with error: agentic pipeline asked a clarifying question: This question requires comparing each Japanese driver's completed lap count against the *winner's* lap count for that same race (a ...

## Judge errors

(none)

## Run failures (no answer; judged INCORRECT)

12 runs ended without an answer: 9 agent asked a clarifying question, 2 max iterations exceeded, 1 generated SQL syntax error. They count against answer accuracy and carry no timing or usage.

## Timing

Wall time per question; shares are of summed wall time across timed runs. LLM, tool, query and startup shares cover all timed runs; `other` (wall - startup - llm - tool - query) is null under fan-out, so it is computed over non-fanout runs only (run count shown). Errored runs carry no timing: counted, not timed. `startup` and `states` are receipt-timed and can lag ~100 ms: use them for ranking.

| db | timed / untimed | wall median s | wall p90 s | LLM | tool | query | startup | other |
|---|---|---|---|---|---|---|---|---|
| financial | 41 / 5 | 41.3 | 123.3 | 96.8% | 1.3% | 1.1% | 0.8% | 0.1% (38 runs) |
| debit_card_specializing | 13 / 0 | 36.1 | 51.5 | 97.6% | 1.1% | 0.6% | 0.5% | 0.2% (13 runs) |
| california_schools | 42 / 6 | 38.2 | 68.4 | 98.6% | 0.8% | 0.5% | 0.1% | 0.1% (40 runs) |
| formula_1 | 54 / 1 | 34.1 | 65.4 | 95.5% | 0.8% | 3.3% | 0.4% | 0.1% (53 runs) |
| **total** | 150 / 12 | 36.6 | 69.2 | 97.5% | 0.9% | 1.2% | 0.3% | 0.1% (144 runs) |

### LLM detail (all dbs)

- calls per question 19.1; mean 5203 ms per call
- TTFT mean 1245 ms and stream mean 2978 ms per measured round; coverage 192 / 2864 rounds (llm_ttft_rounds / llm_calls). Schema rounds and tool-call-only rounds stream no token and are excluded, so ttft + stream is below llm_ms. TTFT is a proxy for network + queueing + prefill and cannot be split further.
- output tokens per question 6468; cache-read share of input 63.7% (cache_read / (cache_read + cache_creation + uncached input))

### Slowest components (summed over all timed runs)

- LLM by state: specifying 11889.7s, clarifying 2192.8s, interpreting 818.0s
- states (receipt-timed): specifying 12026.9s, clarifying 2195.6s, interpreting 818.3s, executing 194.6s
- tools: sample_columns 130.7s (1283 calls, 0 err), search_catalog 3.7s (279 calls, 0 err), compile_semantic_query 2.7s (980 calls, 0 err), check_data_freshness 0.4s (6 calls, 0 err), propose_semantic_query 0.2s (98 calls, 0 err)

### 5 slowest questions

- 12: 3177.6s, dominant llm (3142.3s)
- 13: 3167.4s, dominant llm (3118.9s)
- 227: 488.5s, dominant llm (472.4s)
- 231: 465.1s, dominant llm (458.0s)
- 229: 463.8s, dominant llm (447.4s)

## Bottleneck

Median question takes 36.6s (p90 69.2s). llm is the largest share at 97.5% of wall time (LLM 97.5%, tool 0.9%, query 1.2%, startup 0.3%, other 0.1% over 144 non-fanout runs; 6 fan-out runs). The LLM runs 19.1 rounds per question at 5.2s each. Slowest LLM state: specifying 11889.7s.

Judge-call tokens are not in the agent token counts or the $ column. 12 run failures carry no usage; their entire cost is missing, so $ is a lower bound.

## Run notes

- Regraded 2026-11-18 with `oxy test <db>.test.yml --regrade <archived results> --output-json`
  (commit 419419429; no agent runs, about 160 judge calls). The grader then had the fenced
  judge prompt (90dad1328), the 60 s `statement_timeout` on `bird_ro` (d3c71fdc4), pass/fail
  from the judge verdict rather than EX (3c4c42d05), and errored runs skipping on a failing
  gold (6f2bba893). **No number moved**: 0 of 162 judge verdicts changed and no gold timed
  out, so answer accuracy, EX, Soft-F1, skipped and judge errors are as first reported; only
  the judges' reason wording differs. What did change is each record's `choice`: the first
  run's binary predated 3c4c42d05, so 53 records were judge-CORRECT but `FAIL`; they are now
  `PASS`. The pre-regrade results files are kept outside the repo, in
  `~/Workspace/oxy-evals-archive/bird-2026-11-18-fsm-baseline/`.
- Binary: `target/debug/oxy` built 2026-11-18 12:33 +07, after commit 52323c517 (12:13) and c39927224; so it carries judge-error handling and the timing recorder. Prices are claude-opus-4-8 list prices ($5 / $25 per 1M tokens, claude-api skill cache dated 2026-11-03).
- Run: `cd evals/bird && BIRD_PG_PASSWORD=bird_ro ../../target/debug/oxy test <db>.test.yml --format json --output-json`, sequential. Wall per db: debit_card_specializing 174 s, financial 1037 s, california_schools 3712 s, formula_1 931 s (about 98 min in all). One run per case (`runs: 1`), so every number is a single sample.
- 163 cases ran: 164 Mini-Dev questions minus id 3 (disputed). 162 graded, 1 skipped (debit_card_specializing 149: the connector wraps every query as `SELECT <casts> FROM (<sql>) __q`, and this gold SQL returns two columns named `round`, so the outer reference fails with `42702 column reference "round" is ambiguous`; a grader artefact, not added to disputed.json). Judge model equals the eval model (claude-opus-4-8), so the judge is not independent of the agent.
- Run failures (12) are counted as INCORRECT, so answer accuracy is "of all graded questions", not "of questions the agent answered". Excluding them, answer accuracy is 72/150 = 48.0%.
- Cost: input tokens in the record exclude cache reads and writes, so the $ column prices `cache_read_input_tokens` at 0.1x and `cache_creation_input_tokens` at 1.25x of the input price ($0.50 and $6.25 per 1M). The 1.25x write rate is the 5-minute-TTL rate: the Anthropic client sends `cache_control: {"type": "ephemeral"}` with no `ttl` (crates/agentic/llm/src, checked), so the 5-minute default applies; 1-hour TTL would be 2x. Override with `--price-cache-read` / `--price-cache-write`. The 12 run-failure runs carry no usage at all (no tokens, no cache-token counts), so their entire cost is missing and the $ column is a lower bound (the first version of this report priced only uncached input, $61.60, about half the true figure). Judge-call tokens and cost are not included either.
- Disputed: only id 3 (california_schools; reason "jin et al."). Jin et al. (arXiv 2601.08778, `uiuc-kang-lab/text_to_sql_benchmarks`, `data/arcwise_plat_sql_only_with_diff.json`): 498 questions, 83 carry `original_SQL` (corrected). Their ids are original BIRD dev ids, not our Mini-Dev ids, so we matched on exact question text: 4 of our 164 match, 1 of them (our 3) is corrected (they add `school IS NOT NULL` to both counts). Questions in our set are paraphrased, so exact-text matching undercounts; a fuzzy match was not done. Id 3 was added to disputed.json before the baseline run, so no re-run was needed.
- Triage of the 90 judge-INCORRECT answers (question, gold SQL (first 330 chars) and gold rows read for financial, debit_card_specializing and california_schools 0-41; question and gold rows for the rest): no further gold defect found. Checked and rejected: formula_1 267 (gold rows with NULL driver are what the question asks for when an edition has no qualifier), california_schools 20 (the alphabetical tie-break makes Avetik correct), and zero-count rows in several golds (those are asked for). A hint/SQL contradiction was not found in any of them; the agent answers differ from gold on aggregates, grain or missing rows.
- Not committed: `<db>.test.results.json` (git-ignored). They hold the agent's answers and the gold previews for all cases, more than the committed test files contain.

## Timing observations

- LLM time is 97.5% of wall time; tools 0.9%, warehouse queries 1.2%, startup 0.3%, other 0.1% (non-fanout runs). Almost all LLM time is in the `specifying` state (11,890 s of 14,900 s, 2,272 of 2,864 calls).
- The mean is dominated by two runaway runs: california_schools ids 12 and 13 took 3,177 s and 3,167 s (556 and 819 LLM calls, nearly all in `specifying`). They are 41% of all summed wall time. Over the 150 measured runs (`timing.wall_ms`), the mean wall is 101.9 s with them and 60.4 s without them; the median is 36.6 s either way. The next slowest runs take 420-490 s (60-100 LLM calls).
- Median LLM calls per question is 6, the mean 19.1, the max 819: the call count, not the per-call latency (mean 5.2 s), separates fast from slow questions.
- TTFT coverage is only 192 of 2,864 rounds (6.7%), because most rounds are schema or tool-call-only rounds. Treat the TTFT and stream means as describing that minority of rounds.
- `sample_columns` is the slowest tool (130.7 s over 1,283 calls); no tool reported errors.

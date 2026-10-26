//! Reading a relational query's rows: once, as a stream.
//!
//! The connector used to page an unordered query with `LIMIT 1000 OFFSET n`,
//! re-running it for every page. DuckDB executes a query in parallel, so an
//! unordered result comes back in a different order on each execution: pages
//! overlapped and skipped, and a large result arrived with duplicated and
//! missing rows (5,617 distinct of 5,900 in the test below) — silently, with a
//! plausible row count. The query also ran once per thousand rows. It now runs
//! once, which keeps every row exactly once and the caller's own `ORDER BY`.
//!
//! Rows are handed on as the server sends them rather than collected first, so
//! memory stays flat however large the result, and a consumer that stops
//! reading stops the parse.

use std::future::Future;

use futures::{Stream, StreamExt, stream};

/// Every row of `sql`, with each column cast as `cast_cols` says, from one
/// execution by `run`. `run` sends one statement and returns its messages:
/// `Some(row)` for a row, `None` for anything else (row description, command
/// complete).
pub(super) async fn read_rows<T, E, S, F, Fut>(
    run: F,
    cast_cols: &str,
    sql: &str,
) -> Result<impl Stream<Item = Result<T, E>> + use<T, E, S, F, Fut>, E>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<S, E>>,
    S: Stream<Item = Result<Option<T>, E>>,
{
    let mut messages = Box::pin(run(format!("SELECT {cast_cols} FROM ({sql}) AS _q")).await?);
    // Wait for the first row (or the end), so an error the server reports
    // before any row fails the call, as a structured query error, rather
    // than arriving as the stream's first item.
    let first = loop {
        match messages.next().await {
            Some(Ok(Some(row))) => break Some(row),
            Some(Ok(None)) => {}
            Some(Err(e)) => return Err(e),
            None => break None,
        }
    };
    let rest = messages.filter_map(|msg| std::future::ready(msg.transpose()));
    Ok(stream::iter(first.map(Ok)).chain(rest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    /// A warehouse that, like DuckDB running a query in parallel, returns an
    /// unordered result in a different order on every execution, and honours
    /// LIMIT/OFFSET against that execution's own order.
    struct Shuffling {
        rows: Vec<u32>,
        runs: RefCell<Vec<String>>,
    }

    type Messages = Vec<Result<Option<u32>, String>>;

    impl Shuffling {
        fn new(n: u32) -> Self {
            Self {
                rows: (0..n).collect(),
                runs: RefCell::new(Vec::new()),
            }
        }

        fn run(&self, sql: String) -> Result<Messages, String> {
            let n = self.runs.borrow().len() as u32;
            self.runs.borrow_mut().push(sql.clone());
            let mut rows = self.rows.clone();
            // A different, deterministic permutation per execution.
            rows.sort_by_key(|r| (r.wrapping_mul(2_654_435_761).wrapping_add(n * 97)) % 10_007);
            let limit = sql
                .split(" LIMIT ")
                .nth(1)
                .and_then(|t| t.split_whitespace().next())
                .and_then(|t| t.parse::<usize>().ok());
            let offset = sql
                .split(" OFFSET ")
                .nth(1)
                .and_then(|t| t.parse::<usize>().ok())
                .unwrap_or(0);
            let rows: Vec<u32> = match limit {
                Some(l) => rows.into_iter().skip(offset).take(l).collect(),
                None => rows,
            };
            // Framed like a simple-query response: description, rows, complete.
            Ok(std::iter::once(Ok(None))
                .chain(rows.into_iter().map(|r| Ok(Some(r))))
                .chain(std::iter::once(Ok(None)))
                .collect())
        }
    }

    fn block_on<F: Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    fn all<T, E: std::fmt::Debug>(s: impl Stream<Item = Result<T, E>>) -> Vec<T> {
        block_on(s.map(|r| r.unwrap()).collect())
    }

    #[test]
    fn every_row_comes_back_exactly_once_from_an_unordered_query() {
        let wh = Shuffling::new(5_900);
        let got = all(block_on(read_rows(
            |sql| std::future::ready(wh.run(sql).map(stream::iter)),
            "x",
            "SELECT x FROM t",
        ))
        .unwrap());
        let mut sorted = got.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), got.len(), "no row twice");
        assert_eq!(got.len(), 5_900, "no row missing");
    }

    #[test]
    fn the_query_is_executed_once() {
        let wh = Shuffling::new(2_500);
        all(block_on(read_rows(
            |sql| std::future::ready(wh.run(sql).map(stream::iter)),
            "x",
            "SELECT x FROM t",
        ))
        .unwrap());
        assert_eq!(wh.runs.borrow().len(), 1, "{:?}", wh.runs.borrow());
    }

    #[test]
    fn rows_are_read_only_as_the_consumer_takes_them() {
        let wh = Shuffling::new(5_900);
        let pulled = Cell::new(0usize);
        let rows = block_on(read_rows(
            |sql| {
                std::future::ready(
                    wh.run(sql)
                        .map(|msgs| stream::iter(msgs).inspect(|_| pulled.set(pulled.get() + 1))),
                )
            },
            "x",
            "SELECT x FROM t",
        ))
        .unwrap();
        let first: Vec<u32> = block_on(rows.take(10).map(|r| r.unwrap()).collect());
        assert_eq!(first.len(), 10);
        assert!(
            pulled.get() <= 11,
            "the whole result was read for 10 rows: {} messages",
            pulled.get()
        );
    }

    #[test]
    fn an_error_before_the_first_row_fails_the_call() {
        let msgs: Messages = vec![Ok(None), Err("boom".into())];
        let got = block_on(read_rows(
            |_| std::future::ready(Ok(stream::iter(msgs))),
            "x",
            "SELECT x FROM t",
        ));
        assert_eq!(got.err().as_deref(), Some("boom"));
    }

    #[test]
    fn an_error_after_rows_arrives_in_the_stream() {
        let msgs: Messages = vec![Ok(None), Ok(Some(1)), Ok(Some(2)), Err("boom".into())];
        let rows = block_on(read_rows(
            |_| std::future::ready(Ok(stream::iter(msgs))),
            "x",
            "SELECT x FROM t",
        ))
        .expect("the error comes after the first row");
        let got: Vec<Result<u32, String>> = block_on(rows.collect());
        assert_eq!(got, vec![Ok(1), Ok(2), Err("boom".into())]);
    }
}

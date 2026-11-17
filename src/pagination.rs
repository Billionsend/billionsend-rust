//! Cursor-paginated lists as streams of items.

use std::collections::VecDeque;
use std::pin::Pin;

use futures_util::Stream;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::request::Request;

/// The stream of list items that `into_stream()` returns.
pub type Paginate = Pin<Box<dyn Stream<Item = Result<Value>> + Send>>;

struct State {
    request: Option<Request>,
    backward: bool,
    items: VecDeque<Value>,
    /// An error to end the stream with once `items` are handed out.
    error: Option<Error>,
}

const NO_DATA: &str = "The list page has no data array";

fn page_items(page: Value) -> Result<(Vec<Value>, bool)> {
    let Value::Object(mut page) = page else {
        return Err(Error::application(NO_DATA));
    };
    let has_more = page
        .get("has_more")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match page.remove("data") {
        Some(Value::Array(items)) => Ok((items, has_more)),
        _ => Err(Error::application(NO_DATA)),
    }
}

/// Walks a cursor-paginated list one item at a time, fetching pages on demand.
/// A page without a `data` array ends the stream with an error at once; a page
/// that has more after it but whose edge item (the last one forward, the first
/// one backward) has no non-empty string `id` hands out its items, then ends the stream
/// with an error, since there is no cursor to continue from.
pub(crate) fn paginate(mut request: Request) -> Paginate {
    let backward = request.query_value("before").is_some_and(|v| !v.is_empty());
    if backward {
        request.remove_query("after");
    }
    let state = State {
        request: Some(request),
        backward,
        items: VecDeque::new(),
        error: None,
    };
    Box::pin(futures_util::stream::unfold(
        state,
        |mut state| async move {
            loop {
                if let Some(item) = state.items.pop_front() {
                    return Some((Ok(item), state));
                }
                if let Some(error) = state.error.take() {
                    return Some((Err(error), state));
                }
                let request = state.request.take()?;
                let page = match request
                    .clone()
                    .send_json::<Value>()
                    .await
                    .and_then(page_items)
                {
                    Ok(page) => page,
                    Err(error) => return Some((Err(error), state)),
                };
                let (items, has_more) = page;
                let edge = if state.backward {
                    items.first()
                } else {
                    items.last()
                };
                let cursor = edge
                    .and_then(|item| item.get("id"))
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned);
                if items.is_empty() {
                    return None;
                }
                match (has_more, cursor) {
                    (false, _) => {}
                    (true, Some(cursor)) => {
                        let mut next = request;
                        if state.backward {
                            next.set_query("before", cursor);
                        } else {
                            next.set_query("after", cursor);
                        }
                        state.request = Some(next);
                    }
                    (true, None) => {
                        let edge = if state.backward { "first" } else { "last" };
                        state.error = Some(Error::application(format!(
                            "The {edge} item of the list page has no id to continue from"
                        )));
                    }
                }
                state.items.extend(items);
            }
        },
    ))
}

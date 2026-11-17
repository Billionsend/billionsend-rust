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
}

fn page_items(page: Value) -> Result<(Vec<Value>, bool)> {
    let Value::Object(mut page) = page else {
        return Err(Error::application("A list page is not a JSON object"));
    };
    let has_more = page
        .get("has_more")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match page.remove("data") {
        Some(Value::Array(items)) => Ok((items, has_more)),
        _ => Err(Error::application("A list page has no data array")),
    }
}

/// Walks a cursor-paginated list one item at a time, fetching pages on demand.
pub(crate) fn paginate(mut request: Request) -> Paginate {
    let backward = request.query_value("before").is_some_and(|v| !v.is_empty());
    if backward {
        request.remove_query("after");
    }
    let state = State {
        request: Some(request),
        backward,
        items: VecDeque::new(),
    };
    Box::pin(futures_util::stream::unfold(
        state,
        |mut state| async move {
            loop {
                if let Some(item) = state.items.pop_front() {
                    return Some((Ok(item), state));
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
                    .map(str::to_owned);
                if let (true, Some(cursor)) = (has_more, cursor) {
                    let mut next = request;
                    if state.backward {
                        next.set_query("before", cursor);
                    } else {
                        next.set_query("after", cursor);
                    }
                    state.request = Some(next);
                }
                if items.is_empty() {
                    return None;
                }
                state.items.extend(items);
            }
        },
    ))
}

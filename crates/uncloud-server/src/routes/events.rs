use axum::{
    extract::State,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
};
use futures::stream::Stream;
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio_stream::StreamExt;

use crate::middleware::AuthUser;
use crate::services::events::Event as AppEvent;
use crate::AppState;

pub async fn events_stream(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> impl IntoResponse {
    let receiver = state.events.subscribe(user.id, user.role).await;

    let stream = tokio_stream::wrappers::BroadcastStream::new(receiver)
        .filter_map(|result| {
            result.ok().map(|event| {
                let data = serde_json::to_string(&event).unwrap_or_default();
                Event::default().data(data)
            })
        })
        .map(Ok::<_, Infallible>);

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("ping"),
    )
}

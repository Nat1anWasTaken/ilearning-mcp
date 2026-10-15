//! `/api/tools`: the tool catalog.

use axum::Json;

use crate::{auth::CurrentUser, mcp};

pub async fn list(_: CurrentUser) -> Json<Vec<&'static mcp::Tool>> {
    Json(mcp::all_tools().collect())
}

#[cfg(test)]
mod tests {
    use axum::http::Method;
    use serde_json::json;
    use sqlx::PgPool;

    use crate::testing::{self, dashboard_request, send};

    #[sqlx::test(migrations = "../../migrations")]
    async fn lists_tools(db: PgPool) {
        let state = testing::state(db);
        let (_, cookie) = testing::sign_in(&state, 7).await;
        let app = crate::app(state);

        let tools = send(
            &app,
            dashboard_request(Method::GET, "/api/tools", &cookie, None),
        )
        .await
        .json();
        assert_eq!(tools.as_array().unwrap().len(), 19);
        assert_eq!(
            tools[0],
            json!({
                "name": "get_profile",
                "title": tools[0]["title"],
                "description": tools[0]["description"],
                "kind": "read",
            })
        );
        let submit = tools
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "submit_assignment")
            .unwrap();
        assert_eq!(submit["kind"], "write");
    }
}

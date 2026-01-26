// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

use async_graphql::{EmptySubscription, MergedObject, Schema};
use sqlx::PgPool;

use crate::resolvers::{
    ActionMutation, ActionQuery, IndexingRuleMutation, IndexingRuleQuery, POIDisputeMutation,
    POIDisputeQuery,
};

#[derive(MergedObject, Default)]
pub struct Query(IndexingRuleQuery, ActionQuery, POIDisputeQuery);

#[derive(MergedObject, Default)]
pub struct Mutation(IndexingRuleMutation, ActionMutation, POIDisputeMutation);

pub type ManagementSchema = Schema<Query, Mutation, EmptySubscription>;

/// Build the management API GraphQL schema
pub async fn build_schema(pool: PgPool) -> ManagementSchema {
    Schema::build(Query::default(), Mutation::default(), EmptySubscription)
        .data(pool)
        .finish()
}

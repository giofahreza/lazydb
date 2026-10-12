use sqlparser::{
    ast::{GroupByExpr, ObjectNamePart, OrderByKind, SelectItem, SetExpr, Statement, TableFactor},
    parser::Parser,
};

use super::{SqlDialect, dialect::parser_dialect};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditableResultIdentifier {
    pub value: String,
    pub quoted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditableResultSource {
    pub relation: Vec<EditableResultIdentifier>,
    pub where_clause: Option<String>,
    pub order_by_clause: Option<String>,
    pub limited: bool,
}

pub fn editable_result_source(sql: &str, dialect: SqlDialect) -> Option<EditableResultSource> {
    let statements = Parser::parse_sql(parser_dialect(dialect), sql).ok()?;
    let [Statement::Query(query)] = statements.as_slice() else {
        return None;
    };
    if query.with.is_some()
        || !query.locks.is_empty()
        || query.fetch.is_some()
        || query.for_clause.is_some()
        || query.settings.is_some()
        || query.format_clause.is_some()
        || !query.pipe_operators.is_empty()
    {
        return None;
    }
    let SetExpr::Select(select) = query.body.as_ref() else {
        return None;
    };
    if select.distinct.is_some()
        || select.top.is_some()
        || select.into.is_some()
        || select.from.len() != 1
        || !select.from[0].joins.is_empty()
        || !select.lateral_views.is_empty()
        || select.prewhere.is_some()
        || !select.connect_by.is_empty()
        || !matches!(&select.group_by, GroupByExpr::Expressions(items, modifiers) if items.is_empty() && modifiers.is_empty())
        || !select.cluster_by.is_empty()
        || !select.distribute_by.is_empty()
        || !select.sort_by.is_empty()
        || select.having.is_some()
        || !select.named_window.is_empty()
        || select.qualify.is_some()
        || select.projection.len() != 1
        || !matches!(
            select.projection[0],
            SelectItem::Wildcard(_) | SelectItem::QualifiedWildcard(_, _)
        )
    {
        return None;
    }
    let TableFactor::Table {
        name,
        args,
        with_hints,
        version,
        with_ordinality,
        partitions,
        sample,
        index_hints,
        ..
    } = &select.from[0].relation
    else {
        return None;
    };
    if args.is_some()
        || !with_hints.is_empty()
        || version.is_some()
        || *with_ordinality
        || !partitions.is_empty()
        || sample.is_some()
        || !index_hints.is_empty()
    {
        return None;
    }
    let relation = name
        .0
        .iter()
        .map(|part| match part {
            ObjectNamePart::Identifier(identifier) => Some(EditableResultIdentifier {
                value: identifier.value.clone(),
                quoted: identifier.quote_style.is_some(),
            }),
            ObjectNamePart::Function(_) => None,
        })
        .collect::<Option<Vec<_>>>()?;
    if relation.is_empty() || relation.len() > 3 {
        return None;
    }
    let order_by_clause = match query.order_by.as_ref().map(|order| &order.kind) {
        Some(OrderByKind::Expressions(items)) if !items.is_empty() => Some(
            items
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        ),
        Some(OrderByKind::Expressions(_)) | None => None,
        Some(OrderByKind::All(_)) => return None,
    };
    Some(EditableResultSource {
        relation,
        where_clause: select.selection.as_ref().map(ToString::to_string),
        order_by_clause,
        limited: query.limit_clause.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_simple_wildcard_selects_and_keeps_filters() {
        let source = editable_result_source(
            "select * from public.users where active = true order by id desc limit 10",
            SqlDialect::Postgres,
        )
        .unwrap();
        assert_eq!(
            source
                .relation
                .iter()
                .map(|part| part.value.as_str())
                .collect::<Vec<_>>(),
            ["public", "users"]
        );
        assert_eq!(source.where_clause.as_deref(), Some("active = true"));
        assert_eq!(source.order_by_clause.as_deref(), Some("id DESC"));
        assert!(source.limited);
    }

    #[test]
    fn rejects_results_that_cannot_map_safely_to_one_table() {
        for sql in [
            "select id from users",
            "select * from users join roles using (role_id)",
            "select distinct * from users",
            "select * from users group by id",
            "select * from (select * from users) u",
            "select * from users union select * from admins",
        ] {
            assert_eq!(
                editable_result_source(sql, SqlDialect::Postgres),
                None,
                "{sql}"
            );
        }
    }
}

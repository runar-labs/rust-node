use anyhow::{anyhow, Result};
use base64;
use rusqlite::{Connection, Row, ToSql};
use serde_json::Value;
use std::collections::HashMap;

/// Convert a serde_json::Value to a rusqlite::ToSql implementor
pub fn json_value_to_sql_value(value: &Value) -> Result<Box<dyn ToSql>> {
    match value {
        Value::Null => Ok(Box::new(Option::<String>::None)),
        Value::Bool(b) => Ok(Box::new(*b)),
        Value::Number(n) => {
            if n.is_i64() {
                Ok(Box::new(n.as_i64().unwrap()))
            } else if n.is_f64() {
                Ok(Box::new(n.as_f64().unwrap()))
            } else {
                Ok(Box::new(n.to_string()))
            }
        }
        Value::String(s) => Ok(Box::new(s.clone())),
        _ => Ok(Box::new(value.to_string())),
    }
}

/// Convert a JSON array of parameters to a vector of SQL parameters
pub fn json_params_to_sql_params(params: &Value) -> Result<Vec<Box<dyn ToSql>>> {
    if let Value::Array(array) = params {
        let mut result = Vec::with_capacity(array.len());

        for value in array {
            result.push(json_value_to_sql_value(value)?);
        }

        Ok(result)
    } else {
        Err(anyhow!("Parameters must be a JSON array"))
    }
}

/// Execute a SQL query with JSON parameters
pub fn execute_with_json_params(conn: &Connection, query: &str, params: &Value) -> Result<usize> {
    let param_values = json_params_to_sql_params(params)?;
    let param_refs: Vec<&dyn ToSql> = param_values.iter().map(|p| p.as_ref()).collect();

    let rows_affected = conn.execute(query, rusqlite::params_from_iter(param_refs.iter()))?;
    Ok(rows_affected)
}

/// Map a SQLite row to a JSON object
pub fn map_row_to_json_object(row: &Row, column_names: &[String]) -> Result<Value> {
    let mut obj = serde_json::Map::new();

    for (i, column_name) in column_names.iter().enumerate() {
        let value: Value = match row.get_ref(i)? {
            rusqlite::types::ValueRef::Null => Value::Null,
            rusqlite::types::ValueRef::Integer(i) => Value::Number(i.into()),
            rusqlite::types::ValueRef::Real(f) => {
                // Convert f64 to Value::Number
                let n =
                    serde_json::Number::from_f64(f).unwrap_or_else(|| serde_json::Number::from(0));
                Value::Number(n)
            }
            rusqlite::types::ValueRef::Text(t) => {
                let s = std::str::from_utf8(t).unwrap_or_default();

                // Try to parse as JSON first, if it fails, treat as string
                match serde_json::from_str::<Value>(s) {
                    Ok(json_value) => json_value,
                    Err(_) => Value::String(s.to_string()),
                }
            }
            rusqlite::types::ValueRef::Blob(b) => {
                // Convert blob to base64 string
                Value::String(base64::encode(b))
            }
        };

        obj.insert(column_name.clone(), value);
    }

    Ok(Value::Object(obj))
}

/// Convert a JSON filter object to a SQL WHERE clause and parameters
pub fn json_filter_to_sql_where(
    filter: &Value,
    prefix: Option<&str>,
) -> Result<(String, Vec<Box<dyn ToSql>>)> {
    if !filter.is_object() || filter.as_object().unwrap().is_empty() {
        return Ok((String::new(), Vec::new()));
    }

    let mut where_clause = String::from("WHERE ");
    let mut params: Vec<Box<dyn ToSql>> = Vec::new();
    let mut first = true;

    for (key, value) in filter.as_object().unwrap() {
        if !first {
            where_clause.push_str(" AND ");
        }

        // Handle special operators (like $gt, $lt, etc.)
        if key.starts_with('$') {
            match key.as_str() {
                "$or" => {
                    if let Value::Array(conditions) = value {
                        if conditions.is_empty() {
                            continue;
                        }

                        where_clause.push_str("(");

                        for (i, condition) in conditions.iter().enumerate() {
                            if i > 0 {
                                where_clause.push_str(" OR ");
                            }

                            let (sub_clause, sub_params) =
                                json_filter_to_sql_where(condition, None)?;
                            // Remove the "WHERE" prefix from the sub-clause
                            let sub_clause = if sub_clause.starts_with("WHERE ") {
                                sub_clause[6..].to_string()
                            } else {
                                sub_clause
                            };

                            where_clause.push_str(&format!("({})", sub_clause));
                            params.extend(sub_params);
                        }

                        where_clause.push_str(")");
                    }
                }
                "$and" => {
                    if let Value::Array(conditions) = value {
                        if conditions.is_empty() {
                            continue;
                        }

                        where_clause.push_str("(");

                        for (i, condition) in conditions.iter().enumerate() {
                            if i > 0 {
                                where_clause.push_str(" AND ");
                            }

                            let (sub_clause, sub_params) =
                                json_filter_to_sql_where(condition, None)?;
                            // Remove the "WHERE" prefix from the sub-clause
                            let sub_clause = if sub_clause.starts_with("WHERE ") {
                                sub_clause[6..].to_string()
                            } else {
                                sub_clause
                            };

                            where_clause.push_str(&format!("({})", sub_clause));
                            params.extend(sub_params);
                        }

                        where_clause.push_str(")");
                    }
                }
                _ => return Err(anyhow!("Unsupported operator: {}", key)),
            }
        } else if value.is_object() && !value.as_object().unwrap().is_empty() {
            // This is a field with operators like {field: {$gt: 5}}
            let mut field_clause = String::new();

            // Determine the field name based on whether it's _id or a JSON field
            let field_name = if key == "_id" {
                "_id".to_string()
            } else if let Some(p) = prefix {
                format!("json_extract(data, '$.{}.{}')", p, key)
            } else {
                format!("json_extract(data, '$.{}')", key)
            };

            for (op, op_value) in value.as_object().unwrap() {
                if !field_clause.is_empty() {
                    field_clause.push_str(" AND ");
                }

                match op.as_str() {
                    "$eq" => {
                        field_clause.push_str(&format!("{} = ?", field_name));
                        params.push(json_value_to_sql_value(op_value)?);
                    }
                    "$gt" => {
                        field_clause.push_str(&format!("{} > ?", field_name));
                        params.push(json_value_to_sql_value(op_value)?);
                    }
                    "$gte" => {
                        field_clause.push_str(&format!("{} >= ?", field_name));
                        params.push(json_value_to_sql_value(op_value)?);
                    }
                    "$lt" => {
                        field_clause.push_str(&format!("{} < ?", field_name));
                        params.push(json_value_to_sql_value(op_value)?);
                    }
                    "$lte" => {
                        field_clause.push_str(&format!("{} <= ?", field_name));
                        params.push(json_value_to_sql_value(op_value)?);
                    }
                    "$ne" => {
                        field_clause.push_str(&format!("{} != ?", field_name));
                        params.push(json_value_to_sql_value(op_value)?);
                    }
                    "$in" => {
                        if let Value::Array(values) = op_value {
                            if values.is_empty() {
                                field_clause.push_str("0"); // Never true condition
                            } else {
                                field_clause.push_str(&format!(
                                    "{} IN ({})",
                                    field_name,
                                    values.iter().map(|_| "?").collect::<Vec<_>>().join(", ")
                                ));

                                for val in values {
                                    params.push(json_value_to_sql_value(val)?);
                                }
                            }
                        }
                    }
                    "$nin" => {
                        if let Value::Array(values) = op_value {
                            if values.is_empty() {
                                field_clause.push_str("1"); // Always true condition
                            } else {
                                field_clause.push_str(&format!(
                                    "{} NOT IN ({})",
                                    field_name,
                                    values.iter().map(|_| "?").collect::<Vec<_>>().join(", ")
                                ));

                                for val in values {
                                    params.push(json_value_to_sql_value(val)?);
                                }
                            }
                        }
                    }
                    "$like" => {
                        field_clause.push_str(&format!("{} LIKE ?", field_name));
                        params.push(json_value_to_sql_value(op_value)?);
                    }
                    "$nlike" => {
                        field_clause.push_str(&format!("{} NOT LIKE ?", field_name));
                        params.push(json_value_to_sql_value(op_value)?);
                    }
                    "$regex" => {
                        field_clause.push_str(&format!("{} REGEXP ?", field_name));
                        params.push(json_value_to_sql_value(op_value)?);
                    }
                    "$exists" => {
                        if op_value.as_bool().unwrap_or(false) {
                            field_clause.push_str(&format!("{} IS NOT NULL", field_name));
                        } else {
                            field_clause.push_str(&format!("{} IS NULL", field_name));
                        }
                    }
                    _ => return Err(anyhow!("Unsupported operator: {}", op)),
                }
            }

            where_clause.push_str(&format!("({})", field_clause));
        } else {
            // Simple equality check
            if key == "_id" {
                where_clause.push_str(&format!("_id = ?"));

                if value.is_string() {
                    params.push(json_value_to_sql_value(value)?);
                } else {
                    params.push(Box::new(value.to_string()));
                }
            } else {
                // For other fields, we need to use JSON extraction
                let field_name = if let Some(p) = prefix {
                    format!("json_extract(data, '$.{}.{}')", p, key)
                } else {
                    format!("json_extract(data, '$.{}')", key)
                };

                where_clause.push_str(&format!("{} = ?", field_name));
                params.push(json_value_to_sql_value(value)?);
            }
        }

        first = false;
    }

    Ok((where_clause, params))
}

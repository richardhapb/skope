
use crate::analytics::requests::{AggData, ExecAgg};

struct Database {
    conn: sqlite::Connection,
}

struct Query<T> {
    raw_query: String,
    values: Vec<T>,
}

impl Database {
    pub fn new(name: &str) -> Self {
        let conn = sqlite::open(name).expect("Error generating database");

        Self { conn }
    }

    pub fn insert<T>(&self, storable: T) -> sqlite::Result<()>
    where
        T: Storable,
    {
        let query = storable.build_query();

        // Using a prepared statement to properly handle parameters
        let mut statement = self.conn.prepare(query.raw_query.as_str())?;

        statement.execute(query.values)?;

        Ok(())
    }
}

pub trait Storable {
    type Item;
    fn build_query(&self) -> Query<Self::Item>;
}

impl Storable for ExecAgg {
    type Item = AggData;

    fn build_query(&self) -> Query<Self::Item> {
        let placeholders = self
            .exec_data
            .values()
            .map(|v| format!("({})",
                format!("'{}'", v.name),
                v.total_execs,
                /////////////
        ))
            .collect::<Vec<_>>()
            .join(", ");
        let raw_query = format!(
            "INSERT INTO exec_agg (name, total_execs, total_exec_time, total_memory_usage, avg_exec_memory, avg_exec_time) VALUES ({})",
            placeholders
        );

        Query {
            raw_query,
            values: self.exec_data.values().cloned().collect(),
        }
    }
}

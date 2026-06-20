use std::{collections::{HashMap, HashSet, VecDeque}, time::{SystemTime, UNIX_EPOCH}};

pub enum RedisValue {
    String(String),
    List(VecDeque<String>),
    Hash(HashMap<String, String>),
    Set(HashSet<String>),
}

pub struct Obj {
    pub value: RedisValue,
    pub expires_at: i64,
}

pub struct Store {
    data: HashMap<String,Obj>
}

impl Store {

    pub fn new() -> Self {
        Self {
            data: HashMap::new()
        }
    }

    pub fn set(&mut self, key: String, value: RedisValue, duration_ms: Option<i64>) {
        let expires_at: i64 = match duration_ms {
            Some(ms) => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64;
                now + ms
            },
            None => -1,
        };
        

        let new_obj = Obj {
            value:value,
            expires_at:expires_at
        };

        self.data.insert(key, new_obj);
    }

    pub fn get(&self, key: &str) -> Option<&Obj> {
        let existing = self.data.get(key)?;

        if existing.expires_at != -1 {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            if now > existing.expires_at {
                return None;
            }
        }

        Some(existing)
    }
}
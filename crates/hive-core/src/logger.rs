use crate::identity::now_ms;

pub fn log(category: &str, agent_id: &str, event: &str, detail: &str) {
    let ts = now_ms();
    println!("[{ts}] [{category:8}] {agent_id:16} {event:20} {detail}");
}

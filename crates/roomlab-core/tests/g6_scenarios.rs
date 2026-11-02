//! G6 — every scenario in the pack starts and runs to idle in Demo Mode (§15.2 M acceptance).
mod common;
use common::*;

use roomlab_core::scenarios::scenario_pack;

#[tokio::test]
async fn all_scenarios_run_in_demo_mode() {
    let env = engine();
    env.e.first_run("DEMO").unwrap();
    for s in scenario_pack() {
        let run = env.e.start_scenario(s.id, false).unwrap();
        let room = env.e.read(|c| Ok(roomlab_core::room::list_rooms(c, &run)?[0].id.clone())).unwrap();
        if s.room == roomlab_core::contracts::RoomKind::Story {
            for _ in 0..3 {
                env.e.story_tick(&run, &room).unwrap();
                drain(&env.e, &run).await;
            }
        }
        let reports = drain(&env.e, &run).await;
        let failed = count(&env.e, "SELECT COUNT(*) FROM jobs WHERE run_id=?1 AND status='FAILED'", &run);
        let msgs = count(&env.e, "SELECT COUNT(*) FROM messages WHERE run_id=?1", &run);
        println!("{:<24} jobs_run={:<3} messages={:<3} failed_jobs={failed}", s.id, reports.len(), msgs);
        assert_eq!(failed, 0, "{}", s.id);
        assert!(msgs > 0, "{} produced no messages", s.id);
        assert!(env.e.read(|c| roomlab_core::db::integrity_ok(c)).unwrap());
    }
}

#[tokio::test]
async fn export_sample_for_analysis_script() {
    let env = engine();
    env.e.first_run("DEMO").unwrap();
    let run = env.e.start_scenario("G001_ROLE_ALLOCATION", false).unwrap();
    drain(&env.e, &run).await;
    let req: String = env.e.read(|c| Ok(c.query_row("SELECT request_id FROM spawn_requests WHERE run_id=?1", [&run], |r| r.get(0))?)).unwrap();
    env.e.tx(|c| roomlab_core::spawn::decide(c, &run, &req, true)).unwrap();
    drain(&env.e, &run).await;
    let text = env.e.read(|c| roomlab_core::experiments::export_run(c, &run, roomlab_core::experiments::ExportKind::Operator, &[])).unwrap();
    let path = std::env::temp_dir().join("roomlab_sample_export.json");
    std::fs::write(&path, text).unwrap();
    println!("EXPORT {}", path.display());
}

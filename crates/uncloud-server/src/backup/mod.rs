//! Backup subsystem — `uncloud-server backup {init,create,list,check,prune,restore}`.
//!
//! Writes deduplicated, encrypted snapshots to a Restic-format repository
//! via `rustic_core`. See `docs/backup.md` for the design.

pub mod config;
pub mod lock;

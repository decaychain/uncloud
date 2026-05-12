mod common;

use bson::doc;
use mongodb::bson::oid::ObjectId;
use serde_json::Value;
use uncloud_server::models::{File, Folder};

use common::TestApp;

/// Seed two music-included folders with audio files spread across artists
/// and albums, then verify `GET /api/music/artists` aggregates the rows
/// correctly. Exercises the Mongo-side `$group` rewrite of `list_artists`.
#[tokio::test]
async fn list_artists_aggregates_via_mongo() {
    let app = TestApp::new().await;
    let me: Value = app.register_and_login("alice").await;
    let owner_id = ObjectId::parse_str(me["id"].as_str().expect("user id"))
        .expect("user id is ObjectId");

    let folders = app.db.collection::<Folder>("folders");
    let files = app.db.collection::<File>("files");
    let storage_id = ObjectId::new();

    // Two music-included folders; the second one helps prove the `$in` over
    // multiple parent_ids works.
    let mut music_a = Folder::new(owner_id, None, "Music A".into());
    music_a.music_include = uncloud_common::MusicInclude::Include;
    folders.insert_one(&music_a).await.expect("insert folder A");

    let mut music_b = Folder::new(owner_id, None, "Music B".into());
    music_b.music_include = uncloud_common::MusicInclude::Include;
    folders.insert_one(&music_b).await.expect("insert folder B");

    // A folder that is NOT included — files here must not leak into the
    // artist list, and prove the parent-id filter actually filters.
    let other = Folder::new(owner_id, None, "Documents".into());
    folders.insert_one(&other).await.expect("insert other");

    let make_audio = |parent: ObjectId, name: &str| {
        File::new(
            storage_id,
            format!("alice/{}", name),
            owner_id,
            Some(parent),
            name.to_owned(),
            "audio/mpeg".to_owned(),
            123,
            "deadbeef".to_owned(),
        )
    };
    let mut beatles_white_1 = make_audio(music_a.id, "back-in-the-ussr.mp3");
    beatles_white_1.metadata = audio_meta("The Beatles", "The White Album");
    let mut beatles_white_2 = make_audio(music_a.id, "dear-prudence.mp3");
    beatles_white_2.metadata = audio_meta("The Beatles", "The White Album");
    let mut beatles_abbey = make_audio(music_a.id, "come-together.mp3");
    beatles_abbey.metadata = audio_meta("The Beatles", "Abbey Road");
    let mut bowie_hunky = make_audio(music_b.id, "changes.mp3");
    bowie_hunky.metadata = audio_meta("David Bowie", "Hunky Dory");
    // Empty-string artist + missing album → should bucket as "Unknown".
    let mut blank = make_audio(music_b.id, "unidentified.mp3");
    blank.metadata = audio_meta_blank();
    // Not in any music-included folder; must be ignored.
    let mut excluded = make_audio(other.id, "memo.mp3");
    excluded.metadata = audio_meta("Should Not Appear", "Hidden");

    for f in [
        &beatles_white_1,
        &beatles_white_2,
        &beatles_abbey,
        &bowie_hunky,
        &blank,
        &excluded,
    ] {
        files.insert_one(f).await.expect("insert file");
    }

    let res = app.server.get("/api/music/artists").await;
    res.assert_status_ok();
    let artists: Vec<Value> = res.json();

    // Sorted case-insensitively → "David Bowie", "The Beatles", "Unknown Artist".
    let names: Vec<&str> = artists.iter().map(|a| a["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        vec!["David Bowie", "The Beatles", "Unknown Artist"],
        "artist set / order mismatch: {artists:?}"
    );

    let beatles = artists.iter().find(|a| a["name"] == "The Beatles").unwrap();
    assert_eq!(beatles["album_count"], 2, "Beatles: White Album + Abbey Road");
    assert_eq!(beatles["track_count"], 3);

    let bowie = artists.iter().find(|a| a["name"] == "David Bowie").unwrap();
    assert_eq!(bowie["album_count"], 1);
    assert_eq!(bowie["track_count"], 1);

    let unknown = artists.iter().find(|a| a["name"] == "Unknown Artist").unwrap();
    assert_eq!(unknown["album_count"], 1, "single 'Unknown Album' bucket");
    assert_eq!(unknown["track_count"], 1);

    app.cleanup().await;
}

/// Build a `metadata.audio` map suitable for direct insertion into Mongo.
fn audio_meta(artist: &str, album: &str) -> std::collections::HashMap<String, mongodb::bson::Bson> {
    let mut map = std::collections::HashMap::new();
    map.insert(
        "audio".to_owned(),
        mongodb::bson::Bson::Document(doc! {
            "artist": artist,
            "album": album,
        }),
    );
    map
}

/// Empty-string artist, missing album field — the two paths that should
/// both fall back to the `Unknown ...` defaults.
fn audio_meta_blank() -> std::collections::HashMap<String, mongodb::bson::Bson> {
    let mut map = std::collections::HashMap::new();
    map.insert(
        "audio".to_owned(),
        mongodb::bson::Bson::Document(doc! { "artist": "" }),
    );
    map
}

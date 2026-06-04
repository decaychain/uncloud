mod common;

use bson::doc;
use mongodb::bson::oid::ObjectId;
use serde_json::Value;
use uncloud_server::models::{File, Folder};

use common::TestApp;

/// Seed a music-included folder + a non-included folder, run a search,
/// and verify the three buckets (artists / albums / tracks) plus the
/// `total_*` counters that the `$facet` pipeline now produces.
#[tokio::test]
async fn search_music_buckets_match_against_metadata() {
    let app = TestApp::new().await;
    let me: Value = app.register_and_login("alice").await;
    let owner_id =
        ObjectId::parse_str(me["id"].as_str().expect("user id")).expect("user id is ObjectId");

    let folders = app.db.collection::<Folder>("folders");
    let files = app.db.collection::<File>("files");
    let storage_id = ObjectId::new();

    let mut music = Folder::new(owner_id, None, "Music".into());
    music.music_include = uncloud_common::MusicInclude::Include;
    folders
        .insert_one(&music)
        .await
        .expect("insert music folder");

    let other = Folder::new(owner_id, None, "Documents".into());
    folders.insert_one(&other).await.expect("insert other");

    let audio = |parent: ObjectId, name: &str, artist: &str, album: &str, title: &str| {
        let mut f = File::new(
            storage_id,
            format!("alice/{}", name),
            owner_id,
            Some(parent),
            name.to_owned(),
            "audio/mpeg".to_owned(),
            100,
            "deadbeef".to_owned(),
        );
        f.metadata.insert(
            "audio".to_owned(),
            mongodb::bson::Bson::Document(doc! {
                "artist": artist,
                "album": album,
                "title": title,
            }),
        );
        f
    };

    // Three Beatles tracks across two albums; one Bowie track; one excluded
    // file in `Documents` that must not leak into any bucket regardless of
    // how well it matches.
    let rows = [
        audio(
            music.id,
            "back-in-the-ussr.mp3",
            "The Beatles",
            "The White Album",
            "Back in the USSR",
        ),
        audio(
            music.id,
            "dear-prudence.mp3",
            "The Beatles",
            "The White Album",
            "Dear Prudence",
        ),
        audio(
            music.id,
            "come-together.mp3",
            "The Beatles",
            "Abbey Road",
            "Come Together",
        ),
        audio(
            music.id,
            "changes.mp3",
            "David Bowie",
            "Hunky Dory",
            "Changes",
        ),
        audio(
            other.id,
            "beatles-memo.mp3",
            "The Beatles",
            "Should Not Show",
            "Hidden",
        ),
    ];
    for f in &rows {
        files.insert_one(f).await.expect("insert file");
    }

    // Substring search "beat" — case-insensitive — matches the Beatles
    // artist (3 tracks across 2 albums) but not Bowie. The non-music
    // folder's "beatles-memo" must NOT contribute.
    let res = app.server.get("/api/music/search?q=beat").await;
    res.assert_status_ok();
    let body: Value = res.json();

    let artist_names: Vec<&str> = body["artists"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert_eq!(artist_names, vec!["The Beatles"]);
    assert_eq!(body["artists"][0]["album_count"], 2);
    assert_eq!(body["artists"][0]["track_count"], 3);

    let album_names: Vec<&str> = body["albums"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    // Both Beatles albums matched (sorted by year asc, then name; both have
    // no year so falls back to name ascending case-insensitively).
    assert_eq!(album_names, vec!["Abbey Road", "The White Album"]);

    // Three matching tracks (artist matches every Beatles track), Bowie
    // ignored, excluded folder ignored.
    assert_eq!(body["total_tracks"], 3);
    assert_eq!(body["tracks"].as_array().unwrap().len(), 3);
    // `TrackResponse` flattens `audio` into the top level — `artist` is a
    // direct field on each track row, not nested under `audio`.
    for t in body["tracks"].as_array().unwrap() {
        assert_eq!(t["artist"], "The Beatles");
    }

    // Regex special characters in the query must be escaped, not interpreted
    // — otherwise a query like `(` would 500 the endpoint.
    let res = app.server.get("/api/music/search?q=%28").await; // "("
    res.assert_status_ok();
    let body: Value = res.json();
    assert_eq!(body["total_tracks"], 0);

    app.cleanup().await;
}

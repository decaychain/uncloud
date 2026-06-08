mod common;

use axum::http::StatusCode;
use bson::doc;
use md5::{Digest as Md5Digest, Md5};
use mongodb::bson::oid::ObjectId;
use serde_json::Value;
use uncloud_server::models::{File, Folder};

use common::TestApp;

const TEST_KEY: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";

#[tokio::test]
async fn subsonic_app_password_browses_and_streams_music() {
    let app = TestApp::with_config(|config| {
        config.secrets.master_key = Some(TEST_KEY.to_string());
    })
    .await;
    let me: Value = app.register_and_login("alice").await;
    let owner_id = ObjectId::parse_str(me["id"].as_str().expect("user id")).expect("valid user id");

    let mut folder = Folder::new(owner_id, None, "Music".to_string());
    folder.music_include = uncloud_common::MusicInclude::Include;
    app.db
        .collection::<Folder>("folders")
        .insert_one(&folder)
        .await
        .expect("insert music folder");

    let bytes = b"not really mp3 but good enough for the stream endpoint";
    let uploaded = app
        .upload_to_folder("track.mp3", bytes, "audio/mpeg", &folder.id.to_hex())
        .await;
    let file_id = ObjectId::parse_str(uploaded["id"].as_str().unwrap()).unwrap();
    app.db
        .collection::<File>("files")
        .update_one(
            doc! { "_id": file_id },
            doc! { "$set": { "metadata.audio": {
                "title": "Demo Track",
                "artist": "Demo Artist",
                "album": "Demo Album",
                "duration_secs": 12.0,
                "track_number": 1_i32,
                "year": 2026_i32,
            }}},
        )
        .await
        .expect("update audio metadata");

    let created: Value = app
        .server
        .post("/api/subsonic/credentials")
        .json(&serde_json::json!({ "label": "test client" }))
        .await
        .json();
    let password = created["app_password"].as_str().expect("app password");

    let salt = "abcdef";
    let mut hasher = Md5::new();
    hasher.update(password.as_bytes());
    hasher.update(salt.as_bytes());
    let token = hex::encode(hasher.finalize());

    let base = format!("u=alice&t={token}&s={salt}&v=1.16.1&c=test&f=json");
    let extensions: Value = app
        .server
        .get("/rest/getOpenSubsonicExtensions.view?f=json")
        .await
        .json();
    assert_eq!(extensions["subsonic-response"]["status"], "ok");
    assert_eq!(extensions["subsonic-response"]["openSubsonic"], true);
    assert_eq!(
        extensions["subsonic-response"]["openSubsonicExtensions"][0]["name"],
        "formPost"
    );

    let ping: Value = app
        .server
        .get(&format!("/rest/ping.view?{base}"))
        .await
        .json();
    assert_eq!(ping["subsonic-response"]["status"], "ok");

    let starred: Value = app
        .server
        .get(&format!("/rest/getStarred2.view?{base}"))
        .await
        .json();
    assert_eq!(starred["subsonic-response"]["status"], "ok");
    assert_eq!(
        starred["subsonic-response"]["starred2"]["song"]
            .as_array()
            .expect("starred songs")
            .len(),
        0
    );

    let bookmarks: Value = app
        .server
        .get(&format!("/rest/getBookmarks.view?{base}"))
        .await
        .json();
    assert_eq!(bookmarks["subsonic-response"]["status"], "ok");
    assert_eq!(
        bookmarks["subsonic-response"]["bookmarks"]["bookmark"]
            .as_array()
            .expect("bookmarks")
            .len(),
        0
    );

    let genres: Value = app
        .server
        .get(&format!("/rest/getGenres.view?{base}"))
        .await
        .json();
    assert_eq!(genres["subsonic-response"]["status"], "ok");
    assert_eq!(
        genres["subsonic-response"]["genres"]["genre"]
            .as_array()
            .expect("genres")
            .len(),
        0
    );

    let folders: Value = app
        .server
        .get(&format!("/rest/getMusicFolders.view?{base}"))
        .await
        .json();
    let folder_id = folders["subsonic-response"]["musicFolders"]["musicFolder"][0]["id"]
        .as_str()
        .expect("numeric folder id");
    assert!(folder_id.parse::<i64>().is_ok(), "folder id is numeric");

    let directory: Value = app
        .server
        .get(&format!(
            "/rest/getMusicDirectory.view?{base}&id={folder_id}"
        ))
        .await
        .json();
    let child = &directory["subsonic-response"]["directory"]["child"][0];
    assert_eq!(child["title"], "Demo Track");
    assert!(child.get("coverArt").is_none());
    let song_id = child["id"].as_str().expect("song id");
    assert!(song_id.parse::<i64>().is_ok(), "song id is numeric");

    let album_list: Value = app
        .server
        .get(&format!(
            "/rest/getAlbumList2.view?{base}&type=newest&size=1"
        ))
        .await
        .json();
    let album = &album_list["subsonic-response"]["albumList2"]["album"][0];
    assert_eq!(album["name"], "Demo Album");
    assert!(album.get("coverArt").is_none());
    let album_id = album["id"].as_str().expect("album id");
    let artist_id = album["artistId"].as_str().expect("album artist id");
    assert!(
        artist_id.parse::<i64>().is_ok(),
        "album artist id is numeric"
    );

    let album_detail: Value = app
        .server
        .get(&format!("/rest/getAlbum.view?{base}&id={album_id}"))
        .await
        .json();
    let album_detail = &album_detail["subsonic-response"]["album"];
    assert_eq!(album_detail["name"], "Demo Album");
    assert_eq!(album_detail["artistId"], artist_id);
    assert!(album_detail.get("coverArt").is_none());
    assert!(album_detail["song"][0].get("coverArt").is_none());

    let stream = app
        .server
        .get(&format!("/rest/stream.view?{base}&id={song_id}"))
        .add_header("Range", "bytes=0-6")
        .await;
    stream.assert_status(StatusCode::PARTIAL_CONTENT);
    assert_eq!(stream.as_bytes(), &bytes[..7]);

    app.cleanup().await;
}

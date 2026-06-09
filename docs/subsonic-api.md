# Subsonic API compatibility

This document captures the first implementation scope for exposing Uncloud
Music through the Subsonic API.

## Goals

- Provide a `/rest` compatibility surface for Subsonic/OpenSubsonic clients.
- Use per-user Subsonic app passwords, not the main Uncloud password.
- Stream original files only. Transcoding is deliberately out of scope.
- Reuse the existing Music feature gate and effective music-library visibility.
- Keep Subsonic IDs stable and numeric-looking for broad client compatibility.

## Authentication

Subsonic clients authenticate with `u`, `v`, `c`, and either `p` or `t` + `s`.
Uncloud will store separate per-user Subsonic app passwords encrypted with the
server `secrets.master_key`.

Supported v1 forms:

- `t=md5(password + salt)` with `s=<salt>`
- `p=<password>`
- `p=enc:<hex-encoded password>`

The app password is shown once on creation and can be revoked from Settings.

Client setup notes:

- Feishin must be configured as a Subsonic/OpenSubsonic server, not as a
  Navidrome server. Its Navidrome mode posts to `/auth/login` and expects
  Navidrome's private web API in addition to the Subsonic API.
- The Uncloud Subsonic compatibility surface is `/rest/...`; Uncloud does not
  implement Navidrome's private `/auth` and `/api` music routes.
- `getOpenSubsonicExtensions` is public, as required by OpenSubsonic, and
  currently advertises `formPost` support only.

## IDs

The Subsonic specification treats IDs as strings, but some clients assume they
can be parsed as integers. Uncloud therefore exposes numeric-looking IDs while
treating them as opaque strings.

A dedicated `subsonic_ids` collection maps:

- `owner_id`
- `numeric_id`
- `kind`: `folder`, `song`, `artist`, `album`, `playlist`
- `internal_key`

For real objects, `internal_key` is the Mongo ObjectId string. For synthetic
objects it is a stable key such as `artist:<name>` or
`album:<artist>\0<album>`.

## v1 methods

System:

- `ping`
- `getLicense`
- `getOpenSubsonicExtensions`

Library:

- `getMusicFolders`
- `getIndexes`
- `getMusicDirectory`
- `getArtists`
- `getArtist`
- `getAlbum`
- `getSong`
- `search2`
- `search3`
- `getAlbumList`
- `getAlbumList2`
- `getRandomSongs`
- `getGenres`
- `getStarred`
- `getStarred2`
- `getTopSongs` returns an empty song list in v1.
- `getSimilarSongs` returns an empty song list in v1.
- `getSimilarSongs2` returns an empty song list in v1.
- `getArtistInfo` returns an empty metadata object in v1.
- `getArtistInfo2` returns an empty metadata object in v1.
- `getAlbumInfo` returns an empty metadata object in v1.
- `getAlbumInfo2` returns an empty metadata object in v1.

Media:

- `stream`
- `download`
- `getCoverArt`

Playlists:

- `getPlaylists`
- `getPlaylist`
- `createPlaylist`
- `updatePlaylist`
- `deletePlaylist`

Annotation:

- `scrobble` returns success and does not persist play history in v1.
- `star`, `unstar`, `setRating`, `createBookmark`, and `deleteBookmark`
  return success and do not persist state in v1.
- `getBookmarks` returns an empty list in v1.

Unsupported endpoints should return a normal Subsonic failed response rather
than an Axum 404 whenever the request reached the Subsonic router.

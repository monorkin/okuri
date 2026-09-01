//! The conformance suite, run against real servers.
//!
//! These are ignored by default because they need `test/compose.yaml` to be up. Start it and
//! run them with:
//!
//! ```sh
//! docker compose -f test/compose.yaml up -d
//! cargo test --workspace -- --ignored
//! ```

use std::sync::Arc;

use camion_core::conformance::Conformance;
use camion_core::{RemotePath, Serve};
use camion_providers::destination::{Sftp, SshCredential};
use camion_providers::sftp::SftpProvider;
use camion_providers::trust::TrustEverything;
use camion_providers::Secret;

#[tokio::test]
#[ignore = "needs test/compose.yaml"]
async fn azure_conforms() {
    use camion_providers::azure::AzureProvider;
    use camion_providers::destination::Azure;

    /// The storage emulator's well-known account and key, which are public by design.
    const KEY: &str = "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";

    let config = Azure {
        account: "devstoreaccount1".to_owned(),
        container: "camion-test".to_owned(),
        endpoint: "http://127.0.0.1:10000/devstoreaccount1".to_owned(),
        root: String::new(),
    };

    let provider = AzureProvider::connect(&config, &Secret::Password(KEY.to_owned()))
        .await
        .expect("the compose Azurite container to be running");

    let report = Conformance::new(&provider, RemotePath::parse("/conformance").unwrap())
        .run()
        .await;

    println!("azure skipped: {:?}", report.skipped);
    report.assert_conformant();
}

#[tokio::test]
#[ignore = "needs test/compose.yaml"]
async fn ftp_conforms() {
    use camion_providers::destination::Ftp;
    use camion_providers::ftp::FtpProvider;

    let config = Ftp {
        host: "localhost".to_owned(),
        port: 2121,
        username: "camion".to_owned(),
        // The test server has no certificate. Camion defaults to explicit FTPS everywhere
        // else, and the connection editor says as much before it lets you turn it off.
        encrypted: false,
        passive: true,
        home: String::new(),
    };

    let provider = FtpProvider::connect(&config, &Secret::Password("camion".to_owned()))
        .await
        .expect("the compose FTP server to be running");

    let report = Conformance::new(&provider, RemotePath::parse("/conformance").unwrap())
        .run()
        .await;

    println!("ftp skipped: {:?}", report.skipped);
    report.assert_conformant();
}

#[tokio::test]
#[ignore = "needs test/compose.yaml"]
async fn webdav_conforms() {
    use camion_providers::destination::WebDav;
    use camion_providers::webdav::WebDavProvider;

    let config = WebDav {
        url: "http://localhost:8080".to_owned(),
        username: "camion".to_owned(),
    };

    let provider = WebDavProvider::connect(&config, &Secret::Password("camion".to_owned()))
        .await
        .expect("the compose WebDAV server to be running");

    let report = Conformance::new(&provider, RemotePath::parse("/conformance").unwrap())
        .run()
        .await;

    println!("webdav skipped: {:?}", report.skipped);
    report.assert_conformant();
}

#[tokio::test]
#[ignore = "needs test/compose.yaml"]
async fn s3_conforms() {
    use camion_providers::destination::{S3Preset, S3};
    use camion_providers::s3::S3Provider;

    let config = S3 {
        bucket: "camion-test".to_owned(),
        preset: S3Preset::Other,
        region: "us-east-1".to_owned(),
        endpoint: "http://localhost:9000".to_owned(),
        root: String::new(),
    };

    let provider = S3Provider::connect(
        &config,
        &Secret::KeyPair {
            id: "camion".to_owned(),
            secret: "camion-secret".to_owned(),
        },
    )
    .await
    .expect("the compose MinIO to be running");

    let report = Conformance::new(&provider, RemotePath::parse("/conformance").unwrap())
        .run()
        .await;

    println!("s3 skipped: {:?}", report.skipped);
    report.assert_conformant();
}

/// A refusal has to say what was tried. Asserting the agent is missing when it was never
/// looked for is how someone ends up debugging the wrong thing.
#[tokio::test]
#[ignore = "needs test/compose.yaml"]
async fn sftp_says_what_it_tried_to_sign_in_with() {
    let config = Sftp {
        host: "localhost".to_owned(),
        port: 2222,
        username: "camion".to_owned(),
        credential: SshCredential::Agent,
        home: "/scratch".to_owned(),
    };

    let refused = match SftpProvider::connect(&config, &Secret::None, Arc::new(TrustEverything)).await {
        Err(refused) => refused,
        Ok(_) => panic!("the server accepts only a password, so this should have been refused"),
    };

    let said = refused.to_string();

    // Whatever this machine has, something was attempted and the refusal names it — even
    // "no SSH agent" is an answer. Accepting either wording would let a refusal that explains
    // nothing pass, which is the only thing this test is here to catch.
    assert!(said.contains("camion was not accepted by localhost"), "{said}");
    assert!(said.contains("tried"), "{said}");
    assert!(said.contains("agent"), "{said}");
}

/// What a file is called to the rest of the desktop has to be its whole path. A connection
/// that starts in a home directory would otherwise name files as if they sat at the server's
/// root, and every one of those addresses would point somewhere else.
#[tokio::test]
#[ignore = "needs test/compose.yaml"]
async fn sftp_reports_where_its_root_sits_on_the_server() {
    use camion_core::Provider;

    let config = Sftp {
        host: "localhost".to_owned(),
        port: 2222,
        username: "camion".to_owned(),
        credential: SshCredential::Password,
        // Left empty on purpose: the server decides, and that answer is the one that counts.
        home: String::new(),
    };

    let provider = SftpProvider::connect(
        &config,
        &Secret::Password("camion".to_owned()),
        Arc::new(TrustEverything),
    )
    .await
    .expect("the compose SFTP server to be running");

    // The test server chroots, so its root is the server's. That is reported as nothing at
    // all rather than as "/", so that joining it to a path gives `/file` and not `//file`.
    assert_eq!(provider.home(), "");

    let named = Sftp { home: "/scratch".to_owned(), ..config };
    let anchored = SftpProvider::connect(
        &named,
        &Secret::Password("camion".to_owned()),
        Arc::new(TrustEverything),
    )
    .await
    .expect("the compose SFTP server to be running");

    assert_eq!(anchored.home(), "/scratch");
}

#[tokio::test]
#[ignore = "needs test/compose.yaml"]
async fn sftp_conforms() {
    let config = Sftp {
        host: "localhost".to_owned(),
        port: 2222,
        username: "camion".to_owned(),
        credential: SshCredential::Password,
        home: "/scratch".to_owned(),
    };

    let provider = SftpProvider::connect(
        &config,
        &Secret::Password("camion".to_owned()),
        Arc::new(TrustEverything),
    )
    .await
    .expect("the compose SFTP server to be running");

    let scratch = RemotePath::parse("/conformance").unwrap();

    Conformance::new(&provider, scratch).run().await.assert_conformant();
}

/// What an S3-shaped store will and will not say about one file's readers.
///
/// Worth a real server rather than a unit test: whether a store answers per-file access control
/// at all is the whole question, and no fake can tell you.
#[tokio::test]
#[ignore = "needs test/compose.yaml"]
async fn s3_says_who_can_read_a_file_and_signs_a_link_to_it() {
    use camion_core::{Provider, ProviderExt, Visibility};
    use camion_providers::destination::{S3Preset, S3};
    use camion_providers::s3::S3Provider;

    let config = S3 {
        bucket: "camion-test".to_owned(),
        preset: S3Preset::Other,
        region: "us-east-1".to_owned(),
        endpoint: "http://localhost:9000".to_owned(),
        root: String::new(),
    };

    let provider = S3Provider::connect(
        &config,
        &Secret::KeyPair { id: "camion".to_owned(), secret: "camion-secret".to_owned() },
    )
    .await
    .expect("the compose MinIO to be running");

    let path = RemotePath::parse("/shared.txt").unwrap();
    let contents = b"anyone with the link";

    provider.write_all(&path, bytes::Bytes::from_static(contents)).await.unwrap();

    let sharing = provider.sharing().expect("an S3 connection shares files");

    assert_eq!(sharing.visibility(&path).await.unwrap(), Visibility::Private);

    // The address is where the file lives, so it reads the same whoever is asking.
    assert_eq!(
        sharing.public_url(&path),
        "http://localhost:9000/camion-test/shared.txt"
    );

    // A signed link works without credentials even though the file is private. This is the
    // part that holds on every store, which is why it is the one asserted end to end.
    let link = sharing
        .temporary_url(&path, std::time::Duration::from_secs(600))
        .await
        .unwrap();

    let fetched = reqwest::get(&link).await.unwrap();

    assert!(fetched.status().is_success(), "{}", fetched.status());
    assert_eq!(fetched.bytes().await.unwrap(), &contents[..]);

    // And the plain address does not, because the file is private.
    assert_eq!(reqwest::get(sharing.public_url(&path)).await.unwrap().status(), 403);

    provider.delete(&path).await.unwrap();
}

/// MinIO, Cloudflare R2, and any bucket made since 2023 decide who may read a whole bucket and
/// refuse to talk about one file. What matters is that the refusal says so — read as a generic
/// permissions error it sends you looking at your own credentials.
#[tokio::test]
#[ignore = "needs test/compose.yaml"]
async fn a_store_that_will_not_share_one_file_says_why() {
    use camion_core::{Provider, ProviderExt, Visibility};
    use camion_providers::destination::{S3Preset, S3};
    use camion_providers::s3::S3Provider;

    let config = S3 {
        bucket: "camion-test".to_owned(),
        preset: S3Preset::Other,
        region: "us-east-1".to_owned(),
        endpoint: "http://localhost:9000".to_owned(),
        root: String::new(),
    };

    let provider = S3Provider::connect(
        &config,
        &Secret::KeyPair { id: "camion".to_owned(), secret: "camion-secret".to_owned() },
    )
    .await
    .expect("the compose MinIO to be running");

    let path = RemotePath::parse("/unshareable.txt").unwrap();
    provider.write_all(&path, bytes::Bytes::from_static(b"private")).await.unwrap();

    let sharing = provider.sharing().unwrap();
    let refused = sharing.set_visibility(&path, Visibility::Public).await.unwrap_err();

    assert!(
        refused.to_string().contains("decides who can read a bucket"),
        "{refused}"
    );

    provider.delete(&path).await.unwrap();
}

/// Changing a file's mode, against a real server.
///
/// The interface offers tick boxes for this, and whether they do anything is a question only a
/// server can answer — the mode has to come back changed on the next listing, not merely be
/// accepted.
#[tokio::test]
#[ignore = "needs test/compose.yaml"]
async fn sftp_changes_a_files_mode() {
    use camion_core::{Permissions, Provider, ProviderExt};
    use camion_providers::destination::{SshCredential, Sftp};
    use camion_providers::sftp::SftpProvider;

    let config = Sftp {
        host: "localhost".to_owned(),
        port: 2222,
        username: "camion".to_owned(),
        credential: SshCredential::Password,
        home: "/scratch".to_owned(),
    };

    let provider = SftpProvider::connect(
        &config,
        &Secret::Password("camion".to_owned()),
        Arc::new(TrustEverything),
    )
    .await
    .expect("the compose SFTP server to be running");

    let path = RemotePath::parse("/moded.txt").unwrap();
    provider.write_all(&path, bytes::Bytes::from_static(b"mode")).await.unwrap();

    let permitting = provider.permitting().expect("an SFTP connection keeps modes");

    permitting.set_permissions(&path, Permissions(0o640)).await.unwrap();
    assert_eq!(
        provider.stat(&path).await.unwrap().permissions.unwrap().to_symbolic(),
        "rw-r-----"
    );

    permitting.set_permissions(&path, Permissions(0o644)).await.unwrap();
    assert_eq!(
        provider.stat(&path).await.unwrap().permissions.unwrap().to_symbolic(),
        "rw-r--r--"
    );

    provider.delete(&path).await.unwrap();
}

/// What an object store says about one object, against a real one.
#[tokio::test]
#[ignore = "needs test/compose.yaml"]
async fn s3_says_how_a_file_is_served_and_stored() {
    use camion_core::{Provider, ProviderExt};
    use camion_providers::destination::{S3Preset, S3};
    use camion_providers::s3::S3Provider;

    let config = S3 {
        bucket: "camion-test".to_owned(),
        preset: S3Preset::Other,
        region: "us-east-1".to_owned(),
        endpoint: "http://localhost:9000".to_owned(),
        root: String::new(),
    };

    let provider = S3Provider::connect(
        &config,
        &Secret::KeyPair { id: "camion".to_owned(), secret: "camion-secret".to_owned() },
    )
    .await
    .expect("the compose MinIO to be running");

    let path = RemotePath::parse("/described.txt").unwrap();
    provider.write_all(&path, bytes::Bytes::from_static(b"described")).await.unwrap();

    let served = provider.serving().unwrap().served(&path).await.unwrap();

    // Unquoted: the protocol writes an ETag in quotes and they are not part of the value.
    let etag = served.etag.expect("an ETag");
    assert!(!etag.contains('"'), "{etag}");
    assert_eq!(served.content_type.as_deref(), Some("text/plain"));

    // MinIO says nothing about the class, which by the protocol means the ordinary one.
    let stored = provider.storing().unwrap().stored(&path).await.unwrap();
    assert_eq!(stored.class.as_deref(), Some("STANDARD"));
    assert_eq!(stored.version, None);

    // An SFTP notion, which an object store has no answer for and does not pretend to.
    assert!(provider.owning().is_none());
    assert!(provider.linking().is_none());

    provider.delete(&path).await.unwrap();
}

/// And what a file server says, which is the other half of the vocabulary.
#[tokio::test]
#[ignore = "needs test/compose.yaml"]
async fn sftp_says_who_owns_a_file_and_where_a_link_points() {
    use camion_core::{Provider, ProviderExt};
    use camion_providers::destination::{SshCredential, Sftp};
    use camion_providers::sftp::SftpProvider;

    let config = Sftp {
        host: "localhost".to_owned(),
        port: 2222,
        username: "camion".to_owned(),
        credential: SshCredential::Password,
        home: "/scratch".to_owned(),
    };

    let provider = SftpProvider::connect(
        &config,
        &Secret::Password("camion".to_owned()),
        Arc::new(TrustEverything),
    )
    .await
    .expect("the compose SFTP server to be running");

    let path = RemotePath::parse("/owned.txt").unwrap();
    provider.write_all(&path, bytes::Bytes::from_static(b"owned")).await.unwrap();

    let ownership = provider.owning().unwrap().ownership(&path).await.unwrap();
    assert!(ownership.user.is_some(), "{ownership:?}");
    assert!(ownership.group.is_some(), "{ownership:?}");

    // Not a link, and says so rather than guessing.
    assert_eq!(provider.linking().unwrap().link_target(&path).await.unwrap(), None);

    // An object store's vocabulary, which a file server has no answer for.
    assert!(provider.storing().is_none());

    provider.delete(&path).await.unwrap();
}

/// A file uploaded through Camion has to arrive as what it is.
///
/// Nothing else can put this right afterwards: a store told nothing serves
/// `application/octet-stream` for the life of the object, and every browser downloads it
/// instead of showing it. Both upload paths are checked, because they are two different
/// requests and the large one carries its headers somewhere else entirely.
#[tokio::test]
#[ignore = "needs test/compose.yaml"]
async fn an_uploaded_file_says_what_it_is() {
    use camion_core::{ByteStream, Provider, ProviderExt};
    use camion_providers::destination::{S3Preset, S3};
    use camion_providers::s3::S3Provider;

    let config = S3 {
        bucket: "camion-test".to_owned(),
        preset: S3Preset::Other,
        region: "us-east-1".to_owned(),
        endpoint: "http://localhost:9000".to_owned(),
        root: String::new(),
    };

    let provider = S3Provider::connect(
        &config,
        &Secret::KeyPair { id: "camion".to_owned(), secret: "camion-secret".to_owned() },
    )
    .await
    .expect("the compose MinIO to be running");

    let small = RemotePath::parse("/holiday.jpg").unwrap();
    provider.write_all(&small, bytes::Bytes::from_static(b"not really a jpeg")).await.unwrap();

    assert_eq!(
        provider.serving().unwrap().served(&small).await.unwrap().content_type.as_deref(),
        Some("image/jpeg")
    );

    // Over the part size, so it goes up as a multipart upload — where the type is stated when
    // the upload begins rather than on any of the parts.
    let large = RemotePath::parse("/holiday.png").unwrap();
    let bytes = bytes::Bytes::from(vec![0u8; 9 * 1024 * 1024]);
    provider.write(&large, ByteStream::once(bytes)).await.unwrap();

    assert_eq!(
        provider.serving().unwrap().served(&large).await.unwrap().content_type.as_deref(),
        Some("image/png")
    );

    // A name that says nothing leaves the store to decide, rather than being told wrongly.
    let unknown = RemotePath::parse("/LICENSE").unwrap();
    provider.write_all(&unknown, bytes::Bytes::from_static(b"terms")).await.unwrap();

    assert_eq!(
        provider.serving().unwrap().served(&unknown).await.unwrap().content_type.as_deref(),
        Some("application/octet-stream")
    );

    // Told rather than guessed, which is the case a name cannot cover: a file copied here from
    // another store knows what it is, and plenty of files worth serving have no extension at
    // all. Guessing would make this an octet stream and a browser would download it.
    let told = RemotePath::parse("/zxw70aa0i2orkjdfulmy8ckt7xox").unwrap();
    let serve = Serve {
        content_type: Some("image/png".to_owned()),
        cache_control: Some("public, max-age=3600".to_owned()),
        content_encoding: None,
    };

    provider
        .write(&told, ByteStream::once(&b"not really a png"[..]).served_as(serve))
        .await
        .unwrap();

    let said = provider.serving().unwrap().served(&told).await.unwrap();

    assert_eq!(said.content_type.as_deref(), Some("image/png"));
    assert_eq!(said.cache_control.as_deref(), Some("public, max-age=3600"));

    // And reading it back says the same, off the download itself rather than a second request.
    // This is the other half of copying a file to another store with what it is intact: the
    // read tells us, the write is told, and nothing in between has to ask.
    let coming_back = provider.read(&told, None).await.unwrap();

    assert_eq!(coming_back.serve().content_type.as_deref(), Some("image/png"));
    assert_eq!(
        coming_back.serve().cache_control.as_deref(),
        Some("public, max-age=3600")
    );

    for path in [small, large, unknown, told] {
        provider.delete(&path).await.unwrap();
    }
}

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
use camion_core::RemotePath;
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

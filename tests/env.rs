//! Configuration from the environment. Its own test binary (process), since
//! the other tests build clients that would read these variables too.

mod common;

use billionsend::BillionSend;
use common::{API_KEY, ContractMock};

#[tokio::test]
async fn reads_the_key_and_base_url_from_the_environment() {
    let mock = ContractMock::start().await;
    // SAFETY: the only test of this binary, so nothing reads the environment concurrently.
    unsafe {
        std::env::set_var("BILLIONSEND_API_KEY", API_KEY);
        std::env::set_var("BILLIONSEND_BASE_URL", format!("{}/", mock.uri()));
    }
    let client = BillionSend::from_env();
    unsafe {
        std::env::remove_var("BILLIONSEND_API_KEY");
        std::env::remove_var("BILLIONSEND_BASE_URL");
    }
    assert_eq!(
        client.base_url(),
        mock.uri(),
        "the trailing slash is dropped"
    );
    assert!(client.has_api_key());
    client.domains().list().await.unwrap();
    mock.assert_clean();

    // A blank key counts as none, so the environment's is used; a key from the
    // environment is trimmed too.
    unsafe {
        std::env::set_var("BILLIONSEND_API_KEY", format!("{API_KEY}\n"));
    }
    let blank = mock.builder().api_key(" \n").build().unwrap();
    unsafe {
        std::env::set_var("BILLIONSEND_API_KEY", "  ");
    }
    let none = BillionSend::from_env();
    unsafe {
        std::env::remove_var("BILLIONSEND_API_KEY");
    }
    blank.domains().list().await.unwrap();
    assert!(!none.has_api_key());
    mock.assert_clean();

    let default = BillionSend::builder().build().unwrap();
    assert_eq!(default.base_url(), "https://api.billionsend.com");
}

use talocart_package_fixture::*;
#[tokio::main]
async fn main() -> std::process::ExitCode {
    talocart_package_fixture::main(Variant {
        version: "1.2.1",
        migrations: &[(1, M1), (2, M2), (3, M3), (4, M4_FIXED)],
        serves: true,
    })
    .await
}

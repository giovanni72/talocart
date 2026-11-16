use talocart_package_fixture::*;
#[tokio::main]
async fn main() -> std::process::ExitCode {
    talocart_package_fixture::main(Variant {
        version: "1.0.1",
        migrations: &[(1, M1)],
        serves: false,
    })
    .await
}

//! Builds the vendored ft8_lib (MIT) and Meridian's thin wrapper.
fn main() {
    let root = "third_party/ft8_lib";
    let files = [
        "ft8/constants.c",
        "ft8/crc.c",
        "ft8/decode.c",
        "ft8/encode.c",
        "ft8/ldpc.c",
        "ft8/message.c",
        "ft8/text.c",
        "common/monitor.c",
        "fft/kiss_fft.c",
        "fft/kiss_fftr.c",
        "meridian_ft8.c",
    ];
    let mut build = cc::Build::new();
    // Upstream release flags: -O3 -DHAVE_STPCPY -I.
    build
        .include(root)
        .define("HAVE_STPCPY", None)
        .opt_level(3)
        .warnings(false);
    for f in files {
        build.file(format!("{root}/{f}"));
        println!("cargo:rerun-if-changed={root}/{f}");
    }
    println!("cargo:rerun-if-changed=build.rs");
    build.compile("ft8");
    #[cfg(feature = "soapy")]
    {
        let lib = pkg_config::Config::new()
            .atleast_version("0.8")
            .cargo_metadata(false)
            .probe("SoapySDR")
            .expect("The soapy feature requires the SoapySDR development library");
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
            for path in &lib.link_paths {
                println!("cargo:rustc-link-arg=-Wl,-rpath,{}", path.display());
            }
        }
        println!("cargo:rerun-if-changed=src/bin/soapy/receive.cpp");
        cc::Build::new()
            .cpp(true)
            .std("c++17")
            .includes(lib.include_paths)
            .file("src/bin/soapy/receive.cpp")
            .compile("meridian_soapy");
        pkg_config::Config::new().probe("SoapySDR").unwrap();
    }
}

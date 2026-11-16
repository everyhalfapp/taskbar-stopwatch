// exe 에 아이콘 · 버전 정보 · 매니페스트를 넣는다 (assets/app.rc)
fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    embed_resource::compile("assets/app.rc", embed_resource::NONE);

    // exe 가 직접 불러오는 DLL 을 System32 에서만 찾게 함 (같은 폴더에 둔 가짜 DLL 무시)
    //   0x800 = LOAD_LIBRARY_SEARCH_SYSTEM32, Windows 10 1607 이상에서 적용
    println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
    // Control Flow Guard
    println!("cargo:rustc-link-arg-bins=/GUARD:CF");
}

// exe 에 아이콘 · 버전 정보 · 매니페스트를 넣는다 (assets/app.rc)
fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    embed_resource::compile("assets/app.rc", embed_resource::NONE);
}

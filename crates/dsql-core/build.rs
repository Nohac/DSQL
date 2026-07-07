fn main() {
    println!("cargo:rerun-if-changed=src/grammar/dsql.llw");
    lelwel::build("src/grammar/dsql.llw");
}

fn main() {
    println!("cargo:rerun-if-changed=src/syntax/grammar/dsql.llw");
    lelwel::build("src/syntax/grammar/dsql.llw");
}

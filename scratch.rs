fn main() {
    let label = "8859-1";
    println!("{:?}", encoding_rs::Encoding::for_label(label.as_bytes()).map(|e| e.name()));
    let label = "iso-8859-1";
    println!("{:?}", encoding_rs::Encoding::for_label(label.as_bytes()).map(|e| e.name()));
}

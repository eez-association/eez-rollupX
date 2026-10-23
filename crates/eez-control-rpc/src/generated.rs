//! Build-generated shared wire types/capabilities (v1 package) and the v2
//! proving service. Edit the protobuf sources; Cargo runs protoc for both.
pub mod v1 {
    include!(concat!(env!("OUT_DIR"), "/prove.v1.rs"));
}

pub mod v2 {
    include!(concat!(env!("OUT_DIR"), "/prove.v2.rs"));
}

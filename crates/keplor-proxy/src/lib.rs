//! The proxy core: listener, upstream client, and the body-tee pattern that
//! clones every `Bytes` frame onto a bounded capture channel without
//! buffering the forwarded body.  Filled in from phase 4.

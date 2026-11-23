//! A graph copied from a library stores the full SHA-256 of the entry in its source revision.
//! Existing numeric revisions and graphs without a source keep their saved form. No upgrade
//! step is needed. The last release warns about the newer format and keeps fields it cannot read.

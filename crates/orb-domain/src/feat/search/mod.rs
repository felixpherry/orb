//! Search — finding what was said in orb's threads.
//!
//! orb keeps an index of the prompts the user typed and Claude's text replies
//! from every thread's transcripts, so typed text finds the messages that
//! contain it, newest first, and the exchange each one belongs to.

pub mod index;

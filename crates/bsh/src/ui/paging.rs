//! Background page flow: one read in flight and one page read ahead.
//!
//! A pane's paging state is small enough to be a value: the key of the read it
//! is waiting on, one page buffered for the reader's next reach, and the key of
//! the read-ahead. Keeping it here rather than in the shell lets the coalescing
//! and the one-read-ahead ceiling be tested without a window.
//!
//! The I/O itself is not here - the shell spawns it on the background executor
//! and lands the rows back through the entity. This module only decides whether
//! a request starts, is dropped as a duplicate, or is already buffered.

/// The session and page a read belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PageKey {
    /// The session whose log is being read.
    pub(crate) session: String,
    /// The first row (chat) or page index (trajectory) the read asks for.
    pub(crate) key: usize,
}

/// One pane's paging state.
#[derive(Debug)]
pub(crate) struct PageFlow<T> {
    /// The read the reader is waiting on.
    busy: Option<PageKey>,
    /// A page already read, waiting to be applied when the reader reaches it.
    ahead: Option<T>,
    /// The read-ahead in flight.
    ahead_busy: Option<PageKey>,
    /// Reads started, for the tests: a duplicate request must not move it.
    #[cfg(test)]
    started: usize,
    /// Read-aheads started, for the tests.
    #[cfg(test)]
    ahead_started: usize,
}

impl<T> Default for PageFlow<T> {
    fn default() -> Self {
        Self {
            busy: None,
            ahead: None,
            ahead_busy: None,
            #[cfg(test)]
            started: 0,
            #[cfg(test)]
            ahead_started: 0,
        }
    }
}

impl<T> PageFlow<T> {
    /// Start a foreground read, unless one is already in flight.
    ///
    /// A second scroll while a page is loading is the SAME request: the pane is
    /// already going to get those rows, so the second ask is dropped rather
    /// than queued behind the first.
    pub(crate) fn begin(&mut self, key: PageKey) -> bool {
        if self.busy.is_some() {
            return false;
        }
        self.busy = Some(key);
        #[cfg(test)]
        {
            self.started += 1;
        }
        true
    }

    /// A foreground read finished. True when it was the one in flight.
    pub(crate) fn finish(&mut self, key: &PageKey) -> bool {
        if self.busy.as_ref() != Some(key) {
            return false;
        }
        self.busy = None;
        true
    }

    /// Whether a foreground read is in flight.
    pub(crate) fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    /// Whether a page is already buffered for a reach.
    pub(crate) fn has_ahead(&self) -> bool {
        self.ahead.is_some()
    }

    /// Take the buffered page when it answers this request, leaving it alone
    /// otherwise: a buffered page for a different window is stale.
    pub(crate) fn take_ahead(&mut self, matches: impl FnOnce(&T) -> bool) -> Option<T> {
        if self.ahead.as_ref().is_some_and(matches) {
            self.ahead.take()
        } else {
            None
        }
    }

    /// Drop a buffered page that no longer matches the log.
    pub(crate) fn clear_ahead(&mut self) {
        self.ahead = None;
    }

    /// Start the read-ahead for the next page, unless one is buffered or
    /// already in flight. This is the one-prefetch ceiling.
    pub(crate) fn begin_ahead(&mut self, key: PageKey) -> bool {
        if self.ahead.is_some() || self.ahead_busy.is_some() {
            return false;
        }
        self.ahead_busy = Some(key);
        #[cfg(test)]
        {
            self.ahead_started += 1;
        }
        true
    }

    /// The read-ahead finished. True when it was the one in flight.
    pub(crate) fn finish_ahead(&mut self, key: &PageKey) -> bool {
        if self.ahead_busy.as_ref() != Some(key) {
            return false;
        }
        self.ahead_busy = None;
        true
    }

    /// Buffer a page that finished reading ahead.
    pub(crate) fn store_ahead(&mut self, page: T) {
        self.ahead = Some(page);
    }

    /// Forget everything, because the pane now shows another session.
    pub(crate) fn reset(&mut self) {
        self.busy = None;
        self.ahead = None;
        self.ahead_busy = None;
    }

    /// Foreground reads started, for the coalescing test.
    #[cfg(test)]
    pub(crate) fn started(&self) -> usize {
        self.started
    }

    /// Read-aheads started, for the one-in-flight test.
    #[cfg(test)]
    pub(crate) fn ahead_started(&self) -> usize {
        self.ahead_started
    }
}

#[cfg(test)]
mod tests {
    use super::PageFlow;
    use super::PageKey;

    fn key(session: &str, key: usize) -> PageKey {
        PageKey {
            session: session.to_string(),
            key,
        }
    }

    /// A second scroll while the first page loads is the same request.
    #[test]
    fn a_duplicate_request_coalesces() {
        let mut flow: PageFlow<usize> = PageFlow::default();
        assert!(flow.begin(key("s", 128)));
        assert!(!flow.begin(key("s", 128)), "the same page is not re-read");
        assert!(!flow.begin(key("s", 256)), "nor a different one queued");
        assert_eq!(flow.started(), 1);
        assert!(flow.finish(&key("s", 128)));
        assert!(!flow.is_busy());
        assert!(
            !flow.finish(&key("s", 128)),
            "only the in-flight read lands"
        );
        assert!(flow.begin(key("s", 128)), "the next scroll may start one");
    }

    /// One page may be read ahead, and the ceiling holds while it loads and
    /// while it waits for the reader.
    #[test]
    fn at_most_one_page_is_read_ahead() {
        let mut flow: PageFlow<usize> = PageFlow::default();
        assert!(flow.begin_ahead(key("s", 256)));
        assert!(!flow.begin_ahead(key("s", 256)), "one in flight");
        assert!(flow.finish_ahead(&key("s", 256)));
        flow.store_ahead(256);
        assert!(flow.has_ahead());
        assert!(!flow.begin_ahead(key("s", 128)), "one buffered is enough");
        assert_eq!(flow.take_ahead(|page| *page == 256), Some(256));
        assert!(!flow.has_ahead());
        assert!(flow.begin_ahead(key("s", 128)), "the buffer can refill");
        assert_eq!(flow.ahead_started(), 2);
    }

    /// A buffered page is only consumed by the window it names.
    #[test]
    fn a_stale_buffered_page_is_not_applied() {
        let mut flow: PageFlow<usize> = PageFlow::default();
        flow.store_ahead(256);
        assert_eq!(flow.take_ahead(|page| *page == 128), None);
        assert!(flow.has_ahead());
        flow.clear_ahead();
        assert!(!flow.has_ahead());
    }
}

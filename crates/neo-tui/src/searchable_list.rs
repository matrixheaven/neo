/// Reusable searchable list with fuzzy filtering and pagination.
///
/// Ported from kimi-code `SearchableList<T>`.

/// Options for constructing a [`SearchableList`].
#[derive(Debug, Clone)]
pub struct SearchableListOptions<T> {
    pub items: Vec<T>,
    pub to_search_text: fn(&T) -> String,
    pub page_size: Option<usize>,
    pub initial_index: Option<usize>,
    pub searchable: bool,
}

impl<T> Default for SearchableListOptions<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            to_search_text: |_| String::new(),
            page_size: None,
            initial_index: None,
            searchable: true,
        }
    }
}

/// A read-only view of the current page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchableListView<'a, T> {
    pub items: &'a [T],
    pub selected_index: usize,
    pub page: usize,
    pub page_count: usize,
    pub start: usize,
    pub end: usize,
}

const DEFAULT_PAGE_SIZE: usize = 8;

/// A searchable, paginated list.
#[derive(Debug, Clone)]
pub struct SearchableList<T> {
    items: Vec<T>,
    filtered_indices: Vec<usize>,
    query: String,
    selected: usize,
    page_size: usize,
    searchable: bool,
    to_search_text: fn(&T) -> String,
}

impl<T: PartialEq> PartialEq for SearchableList<T> {
    fn eq(&self, other: &Self) -> bool {
        self.items == other.items
            && self.filtered_indices == other.filtered_indices
            && self.query == other.query
            && self.selected == other.selected
            && self.page_size == other.page_size
            && self.searchable == other.searchable
        // Intentionally skip `to_search_text` — function pointer comparison
        // is unreliable across codegen units.
    }
}

impl<T: Eq> Eq for SearchableList<T> {}

impl<T: Clone + PartialEq> SearchableList<T> {
    #[must_use]
    pub fn new(opts: SearchableListOptions<T>) -> Self {
        let page_size = opts.page_size.unwrap_or(DEFAULT_PAGE_SIZE).max(1);
        let mut list = Self {
            items: opts.items,
            filtered_indices: Vec::new(),
            query: String::new(),
            selected: opts.initial_index.unwrap_or(0),
            page_size,
            searchable: opts.searchable,
            to_search_text: opts.to_search_text,
        };
        list.recompute_filter();
        if list.selected >= list.filtered_indices.len() && !list.filtered_indices.is_empty() {
            list.selected = 0;
        }
        list
    }

    fn recompute_filter(&mut self) {
        if self.query.is_empty() {
            self.filtered_indices = (0..self.items.len()).collect();
            return;
        }
        let q = self.query.to_lowercase();
        let terms: Vec<&str> = q.split_whitespace().collect();
        let mut scored: Vec<(i64, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                let text = (self.to_search_text)(item).to_lowercase();
                let score = fuzzy_score(&text, &terms);
                if score >= 0 { Some((score, i)) } else { None }
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        self.filtered_indices = scored.into_iter().map(|(_, i)| i).collect();
    }

    #[must_use]
    pub fn filtered(&self) -> Vec<&T> {
        self.filtered_indices
            .iter()
            .map(|&i| &self.items[i])
            .collect()
    }

    #[must_use]
    pub fn filtered_items(&self) -> &[T] {
        // Return original items for convenience when no filter
        &self.items
    }

    #[must_use]
    pub fn selected(&self) -> Option<&T> {
        self.filtered_indices
            .get(self.selected)
            .map(|&i| &self.items[i])
    }

    #[must_use]
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    #[must_use]
    pub fn page_size(&self) -> usize {
        self.page_size
    }

    #[must_use]
    pub fn total_filtered(&self) -> usize {
        self.filtered_indices.len()
    }

    pub fn move_up(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        if self.selected == 0 {
            self.selected = self.filtered_indices.len() - 1;
        } else {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.filtered_indices.len();
    }

    pub fn page_up(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let ps = self.page_size;
        if self.selected < ps {
            self.selected = 0;
        } else {
            self.selected -= ps;
        }
    }

    pub fn page_down(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let ps = self.page_size;
        self.selected = (self.selected + ps).min(self.filtered_indices.len() - 1);
    }

    /// Clear the search query. Returns `true` if there was a query to clear.
    pub fn clear_query(&mut self) -> bool {
        if self.query.is_empty() {
            return false;
        }
        self.query.clear();
        self.recompute_filter();
        if self.selected >= self.filtered_indices.len() && !self.filtered_indices.is_empty() {
            self.selected = 0;
        }
        true
    }

    /// Handle a printable character or backspace. Returns `true` if consumed.
    pub fn handle_key(&mut self, data: &str) -> bool {
        if !self.searchable {
            return false;
        }
        match data {
            "backspace" | "\x08" | "\x7f" => {
                if self.query.pop().is_some() {
                    self.recompute_filter();
                    if self.selected >= self.filtered_indices.len() {
                        self.selected = 0;
                    }
                    true
                } else {
                    false
                }
            }
            s if s.chars().all(|c| c.is_ascii_graphic() || c == ' ') && !s.is_empty() => {
                self.query.push_str(s);
                self.recompute_filter();
                if self.selected >= self.filtered_indices.len() {
                    self.selected = 0;
                }
                true
            }
            _ => false,
        }
    }

    #[must_use]
    pub fn view(&self) -> SearchableListView<'_, T> {
        let total = self.filtered_indices.len();
        let page_count = if total == 0 {
            0
        } else {
            (total + self.page_size - 1) / self.page_size
        };
        let current_page = if total == 0 {
            0
        } else {
            self.selected / self.page_size
        };
        let start = current_page * self.page_size;
        let end = (start + self.page_size).min(total);

        SearchableListView {
            items: &self.items,
            selected_index: self.selected,
            page: current_page,
            page_count,
            start,
            end,
        }
    }
}

/// Simple fuzzy score: each term must appear as a subsequence in the text.
/// Returns -1 if not all terms match. Higher score = better match.
fn fuzzy_score(text: &str, terms: &[&str]) -> i64 {
    let mut total_score: i64 = 0;
    for term in terms {
        match subsequence_index(text, term) {
            Some(pos) => {
                // Earlier matches score higher; exact substring bonus
                let bonus: i64 = if text.contains(*term) { 1000 } else { 0 };
                total_score += bonus.saturating_sub(pos as i64);
            }
            None => return -1,
        }
    }
    total_score
}

fn subsequence_index(text: &str, pattern: &str) -> Option<usize> {
    if pattern.is_empty() {
        return Some(0);
    }
    let mut text_chars = text.char_indices().peekable();
    for pattern_char in pattern.chars() {
        let mut found = false;
        for (i, c) in text_chars.by_ref() {
            if c == pattern_char {
                found = true;
                break;
            }
            let _ = i;
        }
        if !found {
            return None;
        }
    }
    Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(items: Vec<&'static str>) -> SearchableListOptions<&'static str> {
        SearchableListOptions {
            items,
            to_search_text: |s| s.to_string(),
            page_size: Some(3),
            initial_index: None,
            searchable: true,
        }
    }

    #[test]
    fn navigation_wraps() {
        let mut list = SearchableList::new(opts(vec!["a", "b", "c"]));
        assert_eq!(list.selected(), Some(&"a"));
        list.move_down();
        assert_eq!(list.selected(), Some(&"b"));
        list.move_down();
        list.move_down();
        assert_eq!(list.selected(), Some(&"a"));
        list.move_up();
        assert_eq!(list.selected(), Some(&"c"));
    }

    #[test]
    fn pagination() {
        let mut list = SearchableList::new(opts(vec!["a", "b", "c", "d", "e"]));
        assert_eq!(list.view().page, 0);
        list.page_down();
        assert_eq!(list.view().page, 1);
        list.page_down();
        assert_eq!(list.view().page, 1); // clamped
        list.page_up();
        assert_eq!(list.view().page, 0);
    }

    #[test]
    fn query_filter() {
        let mut list = SearchableList::new(opts(vec!["apple", "banana", "apricot"]));
        assert!(list.handle_key("ap"));
        assert_eq!(list.total_filtered(), 2); // apple, apricot
    }

    #[test]
    fn clear_query_restores_all() {
        let mut list = SearchableList::new(opts(vec!["apple", "banana"]));
        list.handle_key("xyz");
        assert_eq!(list.total_filtered(), 0);
        assert!(list.clear_query());
        assert_eq!(list.total_filtered(), 2);
        assert!(!list.clear_query()); // already empty
    }

    #[test]
    fn empty_list() {
        let list = SearchableList::new(opts(vec![]));
        assert_eq!(list.selected(), None);
        assert_eq!(list.total_filtered(), 0);
    }

    #[test]
    fn non_searchable_ignores_keys() {
        let mut list = SearchableList::new(SearchableListOptions {
            items: vec!["a", "b"],
            to_search_text: |s| s.to_string(),
            page_size: Some(8),
            initial_index: None,
            searchable: false,
        });
        assert!(!list.handle_key("a"));
    }

    #[test]
    fn backspace_when_empty_query() {
        let mut list = SearchableList::new(opts(vec!["a"]));
        assert!(!list.handle_key("backspace"));
    }
}

(function () {
  const STORAGE_KEY = 'webfind_history';
  const MAX_ITEMS = 15;

  function loadHistory() {
    try {
      return JSON.parse(localStorage.getItem(STORAGE_KEY) || '[]');
    } catch {
      return [];
    }
  }

  function saveHistory(items) {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(items.slice(0, MAX_ITEMS)));
  }

  function renderHistory() {
    const container = document.getElementById('history-sidebar');
    if (!container) return;

    const history = loadHistory();
    if (history.length === 0) {
      container.innerHTML = '<p class="text-xs text-slate-500 p-2">Your recent searches will appear here.</p>';
      return;
    }

    container.innerHTML = history
      .map(
        (q) =>
          `<a href="/web/search?q=${encodeURIComponent(q)}" class="block px-3 py-2 text-sm text-slate-400 hover:text-cyan-400 hover:bg-slate-800 rounded-md transition truncate">${escapeHtml(q)}</a>`
      )
      .join('');
  }

  function addSearch(query) {
    const q = query.trim();
    if (!q) return;

    let history = loadHistory().filter((item) => item !== q);
    history.unshift(q);
    saveHistory(history);
    renderHistory();
  }

  function clearHistory() {
    localStorage.removeItem(STORAGE_KEY);
    renderHistory();
  }

  function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
  }

  // Record searches on form submit
  document.addEventListener('submit', (e) => {
    const form = e.target.closest('form[action="/web/search"]');
    if (!form) return;
    const input = form.querySelector('input[name="q"]');
    if (input) addSearch(input.value);
  });

  // Clear history button
  document.addEventListener('click', (e) => {
    if (e.target.closest('#clear-history')) {
      e.preventDefault();
      clearHistory();
    }
  });

  // Render on load
  document.addEventListener('DOMContentLoaded', renderHistory);
})();

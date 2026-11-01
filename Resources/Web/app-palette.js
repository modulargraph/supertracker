/* Command palette (⌘K): fuzzy search over the command catalogue. */
(function () {
  'use strict';

  const app = window.STApp;
  const { $, $$, esc, openModal, closeModal, commands } = app;
  let paletteIndex = 0;
  let paletteMatches = [];

  function commandSearchText(value) {
    return String(value).normalize('NFKD').replace(/\p{M}/gu, '').toLowerCase().replace(/[^\p{L}\p{N}]+/gu, ' ').trim();
  }

  function commandSearchScore(text, query, words) {
    const normalized = commandSearchText(text),
      tokens = normalized.split(' ');
    if (normalized === query) return 0;
    if (normalized.startsWith(query)) return 10;
    if ((' ' + normalized + ' ').includes(' ' + query + ' ')) return 15;
    // A product version and a numbered item must match separate token occurrences.
    const matches = prefix => {
      const remaining = tokens.slice();
      return words.slice().sort((a, b) => b.length - a.length).every(word => {
        const index = remaining.findIndex(token => prefix ? token.startsWith(word) : token === word);
        if (index < 0) return false;
        remaining.splice(index, 1);
        return true;
      });
    };
    if (matches(false)) return 20;
    if (matches(true)) return 30;
    return Infinity;
  }

  function searchCommands(query) {
    const normalized = commandSearchText(query),
      words = normalized.split(' ');
    if (!normalized) return commands.slice();
    return commands.map((command, index) => ({
      command,
      index,
      score: Math.min(commandSearchScore(command[1], normalized, words),
          ...(command[3] || []).map(alias => 40 + commandSearchScore(alias, normalized, words)))
    })).filter(match => Number.isFinite(match.score)).sort((a, b) => a.score - b.score || a.index - b.index).map(match => match.command);
  }

  function openPalette() {
    paletteIndex = 0;
    openModal('Command palette', '<input id="palette-search" class="palette-input" placeholder="What would you like to do?"'
        + (' autocomplete="off" aria-label="Search commands" role="combobox" aria-autocomplete="list"'
        + ' aria-expanded="true"')
        + ' aria-controls="palette-list"><div id="palette-list" class="palette-list" role="listbox" aria-label="Commands"></div>', '', {
      kind: 'palette',
      className: 'palette',
      noHeader: true
    });
    const search = $('#palette-search');
    if (!search) return;
    search.oninput = () => {
      paletteIndex = 0;
      renderPalette();
    };
    search.onkeydown = e => {
      if (e.isComposing || e.keyCode === 229 || e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        paletteIndex = Math.max(0,
            Math.min(paletteIndex + (e.key === 'ArrowDown' ? 1 : -1), paletteMatches.length - 1));
        renderPalette();
      }
      if (e.key === 'Enter') {
        e.preventDefault();
        const command = paletteMatches[paletteIndex]?.[0];
        if (command) {
          closeModal();
          app.perform(command);
        }
      }
    };
    renderPalette();
  }

  function renderPalette() {
    const search = $('#palette-search'),
      list = $('#palette-list');
    if (!search || !list) return;
    paletteMatches = searchCommands(search.value);
    paletteIndex = Math.max(0, Math.min(paletteIndex, paletteMatches.length - 1));
    list.innerHTML = paletteMatches.length ? paletteMatches.map((c, i) => `<button id="palette-command-${esc(c[0].replace(/:/g, '-'))}" class="palette-item ${i === paletteIndex ? 'active' : ''}"`
        + ` data-command="${esc(c[0])}" role="option" tabindex="-1" aria-selected="${i === paletteIndex}">`
        + `<span>${esc(c[1])}</span><kbd>${esc(c[2])}</kbd></button>`).join('') : '<div class="empty" role="status">No matching command</div>';
    const active = $('.palette-item.active');
    if (active) {
      search.setAttribute('aria-activedescendant', active.id);
    } else {
      search.removeAttribute('aria-activedescendant');
    }
    for (const button of $$('[data-command]')) {
      button.onmousedown = e => e.preventDefault();
      button.onclick = () => {
        closeModal();
        app.perform(button.dataset.command);
      };
    }
    active?.scrollIntoView({ block: 'nearest' });
  }

  Object.assign(app, { commandSearchText, commandSearchScore, searchCommands, openPalette });
})();

// BlockMap — DOM walk → semantic block summary + ASCII outline.
// Replaces visual rendering for LLM page orientation. Cheap O(N) walks.

(function() {
  function divider(n) {
    return new Array((n || 40) + 1).join('─');
  }

  function shortIdent(el) {
    var tag = el.tagName.toLowerCase();
    var id = el.getAttribute('id');
    var cls = el.getAttribute('class');
    var s = tag;
    if (id) s += '#' + id;
    if (cls) {
      var first = cls.split(/\s+/).filter(Boolean).slice(0, 2).join('.');
      if (first) s += '.' + first;
    }
    return s;
  }

  function clean(s) {
    return (s || '').replace(/\s+/g, ' ').trim();
  }

  function attr(el, name) {
    var v = el && el.getAttribute && el.getAttribute(name);
    return v == null || v === '' ? null : String(v);
  }

  function ref(el) {
    return el && el._id ? 'e:' + el._id : null;
  }

  function textOf(node) {
    return clean(node && node.textContent).slice(0, 120);
  }

  function labelText(label, skip) {
    function walk(node) {
      if (!node || node === skip) return '';
      if (node.nodeType === 3) return node.textContent || '';
      if (node.tagName && /^(INPUT|SELECT|TEXTAREA|BUTTON|OPTION)$/.test(node.tagName)) return '';
      var s = '';
      var kids = node.childNodes || [];
      for (var i = 0; i < kids.length; i++) s += ' ' + walk(kids[i]);
      return s;
    }
    return clean(walk(label)).slice(0, 120);
  }

  function labelFor(el) {
    if (!el) return null;
    var aria = attr(el, 'aria-label');
    if (aria) return clean(aria).slice(0, 120);

    var id = attr(el, 'id');
    if (id) {
      var labels = document.getElementsByTagName('label');
      for (var i = 0; i < labels.length; i++) {
        if (labels[i].getAttribute('for') === id) {
          var lt = labelText(labels[i], el) || textOf(labels[i]);
          if (lt) return lt;
        }
      }
    }

    var n = el.parentNode;
    while (n && n.tagName) {
      if (n.tagName === 'LABEL') {
        var wrapped = labelText(n, el) || textOf(n);
        if (wrapped) return wrapped;
      }
      n = n.parentNode;
    }

    var ph = attr(el, 'placeholder');
    if (ph) return clean(ph).slice(0, 120);
    var name = attr(el, 'name');
    if (name) return clean(name).slice(0, 120);
    var title = attr(el, 'title');
    if (title) return clean(title).slice(0, 120);

    var prev = el.previousSibling;
    while (prev) {
      var pt = textOf(prev);
      if (pt) return pt;
      prev = prev.previousSibling;
    }
    return null;
  }

  function controlType(el) {
    var tag = el.tagName.toLowerCase();
    if (tag === 'input') return (el.getAttribute('type') || 'text').toLowerCase();
    if (tag === 'button') return (el.getAttribute('type') || 'submit').toLowerCase();
    return tag;
  }

  function controlValue(el) {
    var tag = el.tagName.toLowerCase();
    if (tag === 'textarea') return el.value != null ? String(el.value) : (el.textContent || '');
    if (tag === 'select') {
      var opts = el.getElementsByTagName('option');
      for (var i = 0; i < opts.length; i++) {
        if (opts[i].selected) return attr(opts[i], 'value') || textOf(opts[i]);
      }
      return opts[0] ? (attr(opts[0], 'value') || textOf(opts[0])) : '';
    }
    if (el.value != null) return String(el.value);
    return attr(el, 'value') || '';
  }

  function optionSamples(select) {
    var opts = select.getElementsByTagName('option');
    var out = [];
    for (var i = 0; i < opts.length && i < 50; i++) {
      out.push({
        ref: ref(opts[i]),
        text: textOf(opts[i]),
        value: attr(opts[i], 'value') || textOf(opts[i]),
        selected: !!opts[i].selected,
      });
    }
    return out;
  }

  function scoreTarget(el, text) {
    var score = 0;
    if (text) score += Math.min(40, text.length);
    if (attr(el, 'aria-label')) score += 35;
    if (attr(el, 'title')) score += 15;
    if (attr(el, 'href') && attr(el, 'href').charAt(0) !== '#') score += 10;
    var role = attr(el, 'role');
    if (role === 'button' || role === 'link') score += 8;
    if (/^(click|here|more|read more|learn more)$/i.test(text || '')) score -= 20;
    if (!text && !attr(el, 'aria-label') && !attr(el, 'title')) score -= 30;
    return score;
  }

  function resolveUrl(url) {
    if (!url) return location.href;
    if (typeof __host_resolve_url === 'function') {
      try { return __host_resolve_url(url, location.href || ''); } catch (e) {}
    }
    if (/^[a-z][a-z0-9+.-]*:/i.test(url)) return url;
    if (url.charAt(0) === '/') return location.origin + url;
    var base = location.href || '';
    return base.slice(0, base.lastIndexOf('/') + 1) + url;
  }

  function isPasswordLike(name, type) {
    return type === 'password' || /pass(word)?|token|secret|credential/i.test(name || '');
  }

  function serializeControl(el) {
    var tag = el.tagName.toLowerCase();
    var type = controlType(el);
    var out = {
      ref: ref(el),
      tag: tag,
      type: type,
      name: attr(el, 'name'),
      label: labelFor(el),
      placeholder: attr(el, 'placeholder'),
      value: controlValue(el),
    };
    if (type === 'checkbox' || type === 'radio') out.checked = !!el.checked;
    if (tag === 'select') {
      out.selected = controlValue(el);
      out.options = optionSamples(el);
    }
    return out;
  }

  function submitReason(el, score) {
    var t = (textOf(el) || attr(el, 'value') || '').toLowerCase();
    var type = controlType(el);
    if (type === 'submit') return 'submit_type';
    if (/search|go|submit|send|apply|continue|next|sign in|login/.test(t)) return 'action_text';
    return score > 0 ? 'button_candidate' : 'low_signal';
  }

  function summarize(el) {
    // Build counts object with only non-zero fields.
    // Downstream Rust consumers read with `.get().unwrap_or(0)`,
    // so missing = zero is safe.
    var _links = el.getElementsByTagName('a').length;
    var _buttons = el.getElementsByTagName('button').length;
    var _inputs = el.querySelectorAll('input, textarea, select').length;
    var _headings = el.querySelectorAll('h1, h2, h3, h4, h5, h6').length;
    var _lists = el.getElementsByTagName('ul').length + el.getElementsByTagName('ol').length;
    var _tables = el.getElementsByTagName('table').length;
    var _images = el.getElementsByTagName('img').length;
    var counts = {};
    if (_links) counts.links = _links;
    if (_buttons) counts.buttons = _buttons;
    if (_inputs) counts.inputs = _inputs;
    if (_headings) counts.headings = _headings;
    if (_tables) counts.tables = _tables;
    if (_lists) counts.lists = _lists;
    if (_images) counts.images = _images;
    // summary kept for ASCII rendering but not included in JSON output
    var _parts = [];
    if (counts.headings) _parts.push(counts.headings + ' headings');
    if (counts.links) _parts.push(counts.links + ' links');
    if (counts.buttons) _parts.push(counts.buttons + ' buttons');
    if (counts.inputs) _parts.push(counts.inputs + ' inputs');
    if (counts.tables) _parts.push(counts.tables + ' tables');
    if (counts.lists) _parts.push(counts.lists + ' lists');
    if (counts.images) _parts.push(counts.images + ' images');
    var _fh = el.querySelectorAll('h1, h2, h3')[0];
    var _firstH = _fh ? clean(_fh.textContent).slice(0, 60) : '';
    var _summary = (_firstH ? '"' + _firstH + '" — ' : '') + (_parts.join(', ') || 'empty');
    return {
      role: el.getAttribute('role') || el.tagName.toLowerCase(),
      ref: 'e:' + el._id,
      ident: shortIdent(el),
      counts: counts,
      _summary: _summary,  // internal only, stripped before return
    };
  }

  function countSelector(root, selector) {
    if (!root || !selector || !selector.trim()) return 0;
    try {
      return root.querySelectorAll(selector).length;
    } catch (e) {
      return 0;
    }
  }

  globalThis.__blockmap = function() {
    var body = document.body;
    if (!body) {
      return {
        title: document.title || '',
        structure: [],
        headings: [],
        interactives: { links: 0, buttons: 0, inputs: [], forms: [] },
        ascii: '(no body)'
      };
    }

    // Headings — v2: single list with chrome flag. Previously had separate
    // `main_headings` which duplicated data. Scan ALL headings first for
    // accurate counts, then emit up to 20, prioritizing content headings
    // over chrome.
    function inChromeAncestor(el) {
      var n = el.parentNode;
      while (n && n.tagName) {
        var t = n.tagName.toLowerCase();
        if (t === 'header' || t === 'nav' || t === 'footer' || t === 'aside') return true;
        n = n.parentNode;
      }
      return false;
    }
    var _allHeadings = [];
    var _hs = body.querySelectorAll('h1, h2, h3, h4, h5, h6');
    for (var _hi = 0; _hi < _hs.length; _hi++) {
      _allHeadings.push({
        level: parseInt(_hs[_hi].tagName[1], 10),
        text: clean(_hs[_hi].textContent).slice(0, 80),
        ref: 'e:' + _hs[_hi]._id,
        chrome: inChromeAncestor(_hs[_hi]),
      });
    }
    // Sort: content headings first, chrome headings after. Then take top 20.
    _allHeadings.sort(function(a, b) { return (a.chrome ? 1 : 0) - (b.chrome ? 1 : 0); });
    var headings = _allHeadings.slice(0, 20);
    // Re-sort by original document order for the agent's readability
    var _refOrder = {};
    for (var _ri = 0; _ri < _hs.length; _ri++) { _refOrder['e:' + _hs[_ri]._id] = _ri; }
    headings.sort(function(a, b) { return (_refOrder[a.ref] || 0) - (_refOrder[b.ref] || 0); });
    // Track counts for downstream consumers (tool likelihood uses main_headings.length)
    var mainHeadingCount = 0;
    for (var _hi2 = 0; _hi2 < headings.length; _hi2++) { if (!headings[_hi2].chrome) mainHeadingCount++; }

    // Interactives
    var links = body.getElementsByTagName('a');
    var buttons = body.getElementsByTagName('button');
    var inputsRaw = body.querySelectorAll('input, textarea, select');
    var inputs = [];
    for (var j = 0; j < inputsRaw.length; j++) {
      var inp = inputsRaw[j];
      inputs.push({
        ref: 'e:' + inp._id,
        tag: inp.tagName.toLowerCase(),
        type: inp.getAttribute('type') || 'text',
        name: inp.getAttribute('name') || null,
        placeholder: inp.getAttribute('placeholder') || null,
        value: inp.getAttribute('value') || null,
      });
    }

    // Helper: strip null fields from an output object to save chars.
    // score is used for ranking but not serialized.
    function _sparse(o, keep) {
      var out = {};
      for (var _k in o) {
        if (_k === 'score') continue;  // ranking-only, never serialize
        if (keep && keep.indexOf(_k) === -1) continue;
        if (o[_k] != null) out[_k] = o[_k];
      }
      return out;
    }

    var linkSamples = [];
    for (var li = 0; li < links.length; li++) {
      var link = links[li];
      var linkText = textOf(link) || labelFor(link) || attr(link, 'title') || attr(link, 'href') || '';
      linkSamples.push({
        ref: ref(link),
        text: linkText,
        href: attr(link, 'href'),
        aria_label: attr(link, 'aria-label'),
        title: attr(link, 'title'),
        role: attr(link, 'role'),
        score: scoreTarget(link, linkText),
      });
    }
    linkSamples.sort(function(a, b) { return b.score - a.score; });
    // v2: cap at 24 and sparsify (drop null fields, drop score)
    linkSamples = linkSamples.slice(0, 24);
    for (var _lsi = 0; _lsi < linkSamples.length; _lsi++) {
      linkSamples[_lsi] = _sparse(linkSamples[_lsi]);
    }

    var buttonEls = [];
    for (var bi = 0; bi < buttons.length; bi++) buttonEls.push(buttons[bi]);
    var inputButtons = body.querySelectorAll('input[type=button], input[type=submit], input[type=reset], input[type=image]');
    for (var ib = 0; ib < inputButtons.length; ib++) buttonEls.push(inputButtons[ib]);
    var buttonSamples = [];
    for (var bs = 0; bs < buttonEls.length; bs++) {
      var btn = buttonEls[bs];
      var btnText = textOf(btn) || attr(btn, 'value') || labelFor(btn) || attr(btn, 'title') || '';
      buttonSamples.push({
        ref: ref(btn),
        text: btnText,
        type: controlType(btn),
        aria_label: attr(btn, 'aria-label'),
        title: attr(btn, 'title'),
        role: attr(btn, 'role'),
        score: scoreTarget(btn, btnText),
      });
    }
    buttonSamples.sort(function(a, b) { return b.score - a.score; });
    // v2: cap at 12, group repeated labels, sparsify
    buttonSamples = buttonSamples.slice(0, 25);  // oversample for grouping
    var _btnGroups = {};
    var _btnOrder = [];
    for (var _bsi = 0; _bsi < buttonSamples.length; _bsi++) {
      var _btn = buttonSamples[_bsi];
      var _bkey = _btn.text + '|' + (_btn.type || '');
      if (!_btnGroups[_bkey]) {
        _btnGroups[_bkey] = { entry: _btn, count: 1 };
        _btnOrder.push(_bkey);
      } else {
        _btnGroups[_bkey].count++;
      }
    }
    buttonSamples = [];
    for (var _boi = 0; _boi < _btnOrder.length && buttonSamples.length < 12; _boi++) {
      var _g = _btnGroups[_btnOrder[_boi]];
      var _sEntry = _sparse(_g.entry);
      if (_g.count > 1) _sEntry.matches = _g.count;
      buttonSamples.push(_sEntry);
    }

    var formEls = body.getElementsByTagName('form');
    var forms = [];
    for (var k = 0; k < formEls.length; k++) {
      var f = formEls[k];
      var controlsRaw = f.querySelectorAll('input, textarea, select, button');
      var controls = [];
      var submitCandidates = [];
      var previewFields = [];
      var method = (f.getAttribute('method') || 'get').toLowerCase();
      for (var ci = 0; ci < controlsRaw.length; ci++) {
        var control = controlsRaw[ci];
        var ctl = serializeControl(control);
        controls.push(ctl);

        var ctype = ctl.type;
        var isSubmit = (control.tagName === 'BUTTON' && ctype !== 'button' && ctype !== 'reset') ||
          (control.tagName === 'INPUT' && (ctype === 'submit' || ctype === 'image'));
        if (isSubmit) {
          var st = textOf(control) || attr(control, 'value') || labelFor(control) || '';
          var ss = scoreTarget(control, st) + (ctype === 'submit' ? 30 : 0);
          submitCandidates.push({
            ref: ref(control),
            tag: control.tagName.toLowerCase(),
            text: st,
            type: ctype,
            score: ss,
            reason: submitReason(control, ss),
          });
        }

        if (method === 'get' && ctl.name && ctype !== 'submit' && ctype !== 'button' && ctype !== 'reset' && ctype !== 'image') {
          if ((ctype === 'checkbox' || ctype === 'radio') && !ctl.checked) continue;
          previewFields.push({
            name: ctl.name,
            value: isPasswordLike(ctl.name, ctype) ? '[REDACTED]' : (ctl.value || ''),
            type: ctype,
            redacted: isPasswordLike(ctl.name, ctype),
          });
        }
      }
      submitCandidates.sort(function(a, b) { return b.score - a.score; });
      var action = f.getAttribute('action') || location.href || '';
      forms.push({
        ref: ref(f),
        action: f.getAttribute('action') || '',
        method: method,
        fields: f.querySelectorAll('input, textarea, select').length,
        controls: controls,
        submit_candidates: submitCandidates.slice(0, 10),
        query_preview: method === 'get' ? {
          action: resolveUrl(action),
          fields: previewFields,
        } : null,
      });
    }

    // Stable selector hints are concrete, page-local signals that help agents
    // choose between CSS querying and text/extract fallbacks. `role` here is
    // explicit only; HTML's implicit semantic roles are not counted.
    var contentRoot = document.querySelector('main, [role="main"], article, #root, #app') || body;
    var selectors = {
      data_testid: countSelector(contentRoot, '[data-testid]'),
      aria_label: countSelector(contentRoot, '[aria-label]'),
      role: countSelector(contentRoot, '[role]'),
    };

    // Structure: HTML5 landmarks first; fall back to significant top-level children.
    var structure = [];
    var landmarks = body.querySelectorAll('header, nav, main, aside, footer, article, section');
    for (var m = 0; m < landmarks.length; m++) {
      structure.push(summarize(landmarks[m]));
    }
    if (structure.length === 0) {
      var children = body.children;
      for (var c = 0; c < children.length; c++) {
        var ch = children[c];
        if (ch.getElementsByTagName('*').length >= 5) {
          structure.push(summarize(ch));
        }
      }
    }

    // RLE grouping: collapse repeated shapes (same role + ident + counts) to
    // keep up to 3 examples + a repeat count. Saves ~5KB on card-grid pages.
    var _shapeFreq = {};
    for (var _si = 0; _si < structure.length; _si++) {
      var _sk = structure[_si].role + '|' + structure[_si].ident + '|' + JSON.stringify(structure[_si].counts);
      _shapeFreq[_sk] = (_shapeFreq[_sk] || 0) + 1;
    }
    var _shapeSeen = {};
    var _compactStructure = [];
    for (var _si2 = 0; _si2 < structure.length; _si2++) {
      var _block = structure[_si2];
      var _sk2 = _block.role + '|' + _block.ident + '|' + JSON.stringify(_block.counts);
      var _n = (_shapeSeen[_sk2] || 0) + 1;
      _shapeSeen[_sk2] = _n;
      if (_n <= 3 || _shapeFreq[_sk2] <= 3) {
        // Emit up to 3 examples of each shape
        _compactStructure.push(_block);
      }
      // On the 3rd emit (or last if fewer than 3 total examples), tag with repeat
      if (_n === Math.min(3, _shapeFreq[_sk2])) {
        if (_shapeFreq[_sk2] > 3) {
          _block.repeat = _shapeFreq[_sk2];
        }
      }
    }
    structure = _compactStructure;

    // ASCII outline
    var ascii = [];
    var bar = '  ' + divider(64);
    ascii.push('  ' + (document.title || '(untitled)'));
    ascii.push(bar);
    if (structure.length === 0) {
      ascii.push('  (no landmarks or significant top-level blocks)');
    } else {
      for (var s = 0; s < structure.length; s++) {
        var b = structure[s];
        var role = (b.role.toUpperCase() + '          ').slice(0, 9);
        var _rep = b.repeat ? ' ×' + b.repeat : '';
        ascii.push('  ' + role + ' [' + b.ref + '] ' + b.ident + _rep + ' — ' + b._summary);
      }
    }
    ascii.push(bar);
    if (headings.length) {
      ascii.push('  HEADINGS (' + headings.length + ')');
      for (var h = 0; h < headings.length && h < 8; h++) {
        var indent = new Array(headings[h].level + 1).join(' ');
        ascii.push('    ' + indent + 'h' + headings[h].level + ' ' + headings[h].text);
      }
    }
    ascii.push('  INTERACTIVES: ' + links.length + ' links · ' + buttons.length + ' buttons · ' + inputs.length + ' inputs · ' + forms.length + ' forms');

    // Data-density signal: distinguishes "fully SSR'd" pages from "SSR shell
    // with JS-populated cells" (e.g. CNBC tables, financial dashboards). Three
    // signals, OR'd: empty <td>s, empty <li>s, or empty <table> shells (the
    // worst case — page has table tags but rows/cells get JS-injected, so no
    // <td> exists at all in the static HTML).
    function densityOf(els, threshold) {
      if (!els || els.length === 0) return null;
      var filled = 0;
      var minLen = threshold || 2;
      for (var di = 0; di < els.length; di++) {
        var t = (els[di].textContent || '').replace(/\s+/g, ' ').trim();
        if (t.length >= minLen) filled++;
      }
      var ratio = filled / els.length;
      return {
        total: els.length,
        filled: filled,
        ratio: Math.round(ratio * 1000) / 1000,
      };
    }
    var tdDensity = densityOf(body.getElementsByTagName('td'), 2);
    var liDensity = densityOf(body.getElementsByTagName('li'), 2);
    // For tables, "empty" = under 5 chars of textContent (the table tag itself
    // and whitespace). Threshold higher because tables have wrapper noise.
    var tableDensity = densityOf(body.getElementsByTagName('table'), 5);

    function suspicious(d, minTotal) {
      return d != null && d.total >= (minTotal || 20) && d.ratio < 0.4;
    }

    // Thin-shell signal: page is small, structure is empty, no headings, few links.
    // Catches the crates.io / DDG-main class of SPA where the static HTML is just
    // a React/Ember root and a script tag. The skill markdown described this
    // heuristic but it lived in agent prose only — now computed inline so every
    // caller benefits.
    var bodyBytes = (document.body && (document.body.textContent || '').length) || 0;
    // Use a rough proxy for "page bytes" — actual response body length isn't
    // available JS-side. innerText length is a reasonable lower bound.
    var thinShell =
      structure.length < 3 &&
      headings.length === 0 &&
      links.length < 30 &&
      bodyBytes < 4000;

    var allScripts = document.querySelectorAll('script');
    // Many normal SSR pages carry 15+ analytics/chunk scripts. Treat this as
    // a shell only when visible content and semantic structure are both sparse.
    var scriptHeavyShell =
      allScripts.length >= 20 &&
      structure.length <= 1 &&
      mainHeadingCount === 0 &&
      links.length < 20 &&
      bodyBytes < 6000;

    var likelyJsFilled =
      suspicious(tdDensity, 20) ||
      suspicious(liDensity, 30) ||
      suspicious(tableDensity, 3) ||   // even a few empty tables is a strong signal
      thinShell ||                      // SPA shell with no rendered content
      scriptHeavyShell;                 // large app shell with scripts but no visible UI

    // JSON-bearing script tags often carry the data the JS rendering would
    // fill in. Beyond the standard application/json + application/ld+json,
    // commerce platforms use custom MIME-like types: text/x-magento-init,
    // text/x-shopify-app, application/vnd.shopify.product+json, etc. Count
    // all of them so the density signal accurately predicts whether
    // extract() will find structured data.
    var jsonScripts = 0;
    for (var jsIdx = 0; jsIdx < allScripts.length; jsIdx++) {
      var jsType = (allScripts[jsIdx].getAttribute('type') || '').toLowerCase();
      if (jsType.indexOf('json') !== -1 ||
          jsType.indexOf('x-magento') !== -1 ||
          jsType.indexOf('x-shopify') !== -1 ||
          jsType.indexOf('x-component') !== -1) {
        jsonScripts++;
      }
    }

    // Fold into the ASCII summary.
    var hasDensity = tdDensity || liDensity || tableDensity;
    if (hasDensity) {
      var densityLine = '  DATA DENSITY:';
      if (tableDensity) densityLine += ' tables=' + tableDensity.filled + '/' + tableDensity.total;
      if (tdDensity)    densityLine += ' td=' + tdDensity.filled + '/' + tdDensity.total + ' (' + Math.round(tdDensity.ratio * 100) + '%)';
      if (liDensity)    densityLine += ' li=' + liDensity.filled + '/' + liDensity.total + ' (' + Math.round(liDensity.ratio * 100) + '%)';
      if (likelyJsFilled) densityLine += '  ⚠ likely JS-filled (cells empty)';
      ascii.push(densityLine);
    }
    if (jsonScripts > 0) {
      ascii.push('  JSON SCRIPTS: ' + jsonScripts + ' (data may be embedded — try `extract()` first, it covers ld+json / __NEXT_DATA__ / Magento / Shopify)');
    }
    if (scriptHeavyShell) {
      ascii.push('  SCRIPT SHELL: ' + allScripts.length + ' scripts with little visible content — likely browser-rendered');
    }
    if (selectors.data_testid || selectors.aria_label || selectors.role) {
      ascii.push('  SELECTOR HINTS: data-testid=' + selectors.data_testid + ' aria=' + selectors.aria_label + ' role=' + selectors.role);
    }

    return {
      blockmap_version: 2,
      title: document.title || '',
      structure: structure,
      headings: headings,
      selectors: selectors,
      interactives: {
        links: links.length,
        buttons: buttons.length,
        link_samples: linkSamples,
        button_samples: buttonSamples,
        inputs: inputs,
        forms: forms,
      },
      density: {
        tables: tableDensity,
        td: tdDensity,
        li: liDensity,
        json_scripts: jsonScripts,
        script_tags: allScripts.length,
        body_text_chars: bodyBytes,
        script_heavy_shell: scriptHeavyShell,
        thin_shell: thinShell,
        likely_js_filled: likelyJsFilled,
      },
      ascii: ascii.join('\n'),
    };
  };
})();

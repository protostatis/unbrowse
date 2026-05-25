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
    var counts = {
      links: el.getElementsByTagName('a').length,
      buttons: el.getElementsByTagName('button').length,
      inputs: el.querySelectorAll('input, textarea, select').length,
      headings: el.querySelectorAll('h1, h2, h3, h4, h5, h6').length,
      lists: el.getElementsByTagName('ul').length + el.getElementsByTagName('ol').length,
      tables: el.getElementsByTagName('table').length,
      images: el.getElementsByTagName('img').length,
    };
    var parts = [];
    if (counts.headings) parts.push(counts.headings + ' headings');
    if (counts.links) parts.push(counts.links + ' links');
    if (counts.buttons) parts.push(counts.buttons + ' buttons');
    if (counts.inputs) parts.push(counts.inputs + ' inputs');
    if (counts.tables) parts.push(counts.tables + ' tables');
    if (counts.lists) parts.push(counts.lists + ' lists');
    if (counts.images) parts.push(counts.images + ' images');
    var firstHeading = '';
    var fh = el.querySelectorAll('h1, h2, h3')[0];
    if (fh) firstHeading = clean(fh.textContent).slice(0, 60);
    return {
      role: el.getAttribute('role') || el.tagName.toLowerCase(),
      ref: 'e:' + el._id,
      ident: shortIdent(el),
      counts: counts,
      summary: (firstHeading ? '"' + firstHeading + '" — ' : '') + (parts.join(', ') || 'empty'),
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

    // Headings — keep up to 20. Also surface a `main_headings` list that
    // excludes anything inside <header>/<nav>/<footer>/<aside>, because on
    // sites like GitHub the global headings list is dominated by site chrome
    // ("Navigation Menu", "Search code...", etc.) instead of the actual
    // page topic.
    function inChromeAncestor(el) {
      var n = el.parentNode;
      while (n && n.tagName) {
        var t = n.tagName.toLowerCase();
        if (t === 'header' || t === 'nav' || t === 'footer' || t === 'aside') return true;
        n = n.parentNode;
      }
      return false;
    }
    var headings = [];
    var mainHeadings = [];
    var hs = body.querySelectorAll('h1, h2, h3, h4, h5, h6');
    for (var i = 0; i < hs.length && i < 20; i++) {
      var entry = {
        level: parseInt(hs[i].tagName[1], 10),
        text: clean(hs[i].textContent).slice(0, 80),
        ref: 'e:' + hs[i]._id,
      };
      headings.push(entry);
      if (!inChromeAncestor(hs[i])) mainHeadings.push(entry);
    }

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
    linkSamples = linkSamples.slice(0, 50);

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
    buttonSamples = buttonSamples.slice(0, 50);

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
        ascii.push('  ' + role + ' [' + b.ref + '] ' + b.ident + ' — ' + b.summary);
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
      mainHeadings.length === 0 &&
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
      title: document.title || '',
      structure: structure,
      headings: headings,
      main_headings: mainHeadings,
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

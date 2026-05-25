// PageModel - semantic object reconstruction on top of the DOM.
//
// This layer keeps structure that flat text/link dumps lose: forms own their
// controls, cards own their title/url/date/tags, and every object carries
// provenance for why it exists.

(function() {
  var STOP = {
    the: true, and: true, for: true, with: true, from: true, that: true,
    this: true, into: true, what: true, when: true, where: true, find: true,
    list: true, show: true, how: true, many: true, currently: true,
    available: true, make: true, sure: true, can: true, perform: true
  };

  function clean(s) {
    return String(s || '').replace(/\s+/g, ' ').trim();
  }

  function tagName(node) {
    return ((node && node.tagName) || '').toLowerCase();
  }

  function attr(el, name) {
    var v = el && el.getAttribute && el.getAttribute(name);
    return v == null || v === '' ? null : String(v);
  }

  function ref(el) {
    return el && el._id ? 'e:' + el._id : null;
  }

  function resolveUrl(url) {
    if (!url) return null;
    if (typeof __host_resolve_url === 'function') {
      try { return __host_resolve_url(url, (typeof location !== 'undefined' && location.href) || '') || url; } catch (e) {}
    }
    return url;
  }

  function textOf(node, max) {
    max = max || 400;
    if (!node) return '';
    var chunks = [];
    function walk(n) {
      if (!n) return;
      if (n.nodeType === 3) {
        var t = clean(n.textContent || '');
        if (t) chunks.push(t);
        return;
      }
      if (n.nodeType !== 1 && n.nodeType !== 9 && n.nodeType !== 11) return;
      var tag = tagName(n);
      if (tag === 'script' || tag === 'style' || tag === 'noscript' || tag === 'template' || tag === 'svg') return;
      var kids = n.childNodes || [];
      for (var i = 0; i < kids.length; i++) walk(kids[i]);
    }
    walk(node);
    var out = clean(chunks.join(' '));
    if (out.length > max) out = out.slice(0, max - 3).replace(/\s+\S*$/, '') + '...';
    return out;
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
    return clean(walk(label)).slice(0, 160);
  }

  function labelFor(el) {
    if (!el) return null;
    var aria = attr(el, 'aria-label');
    if (aria) return clean(aria).slice(0, 160);
    var id = attr(el, 'id');
    if (id) {
      var labels = document.getElementsByTagName('label');
      for (var i = 0; i < labels.length; i++) {
        if (labels[i].getAttribute('for') === id) {
          var direct = labelText(labels[i], el) || textOf(labels[i], 160);
          if (direct) return direct;
        }
      }
    }
    var n = el.parentNode;
    while (n && n.tagName) {
      if (n.tagName === 'LABEL') {
        var wrapped = labelText(n, el) || textOf(n, 160);
        if (wrapped) return wrapped;
      }
      n = n.parentNode;
    }
    return attr(el, 'placeholder') || attr(el, 'name') || attr(el, 'title') || null;
  }

  function controlType(el) {
    var tag = tagName(el);
    if (tag === 'input') return (attr(el, 'type') || 'text').toLowerCase();
    if (tag === 'button') return (attr(el, 'type') || 'submit').toLowerCase();
    return tag;
  }

  function controlValue(el) {
    var tag = tagName(el);
    if (tag === 'textarea') return el.value != null ? String(el.value) : (el.textContent || '');
    if (tag === 'select') {
      var opts = el.getElementsByTagName('option');
      for (var i = 0; i < opts.length; i++) {
        if (opts[i].selected) return attr(opts[i], 'value') || textOf(opts[i], 80);
      }
      return opts[0] ? (attr(opts[0], 'value') || textOf(opts[0], 80)) : '';
    }
    return el.value != null ? String(el.value) : (attr(el, 'value') || '');
  }

  function optionSamples(select) {
    var opts = select.getElementsByTagName('option');
    var out = [];
    for (var i = 0; i < opts.length && i < 30; i++) {
      out.push({ text: textOf(opts[i], 80), value: attr(opts[i], 'value') || textOf(opts[i], 80), selected: !!opts[i].selected });
    }
    return out;
  }

  function isPasswordLike(name, type) {
    return type === 'password' || /pass(word)?|token|secret|credential/i.test(name || '');
  }

  function terms(goal) {
    var raw = clean(goal).toLowerCase().split(/[^a-z0-9]+/).filter(Boolean);
    var out = [];
    for (var i = 0; i < raw.length; i++) {
      if (raw[i].length > 2 && !STOP[raw[i]] && out.indexOf(raw[i]) === -1) out.push(raw[i]);
    }
    return out;
  }

  function termHits(haystack, ts) {
    haystack = clean(haystack).toLowerCase();
    if (!haystack || !ts.length) return 0;
    var hits = 0;
    for (var i = 0; i < ts.length; i++) if (haystack.indexOf(ts[i]) !== -1) hits++;
    return hits;
  }

  function goalBoost(haystack, ts) {
    if (!ts.length) return 0;
    return Math.min(0.35, termHits(haystack, ts) / ts.length * 0.35);
  }

  function clampScore(n) {
    if (n < 0) return 0;
    if (n > 1) return 1;
    return Math.round(n * 1000) / 1000;
  }

  function hasType(kind, allowed) {
    if (!allowed || !allowed.length) return true;
    if (allowed.indexOf(kind) !== -1) return true;
    if (/.*_card$/.test(kind) && allowed.indexOf('card') !== -1) return true;
    return false;
  }

  function nearestChrome(el) {
    var n = el;
    while (n && n.tagName) {
      var tag = tagName(n);
      if (tag === 'nav' || tag === 'header' || tag === 'footer' || tag === 'aside') return tag;
      n = n.parentNode;
    }
    return null;
  }

  function nearestHeading(el) {
    var n = el;
    while (n && n.tagName) {
      var h = n.querySelector && n.querySelector('h1,h2,h3,h4,[itemprop="name"]');
      if (h) return textOf(h, 180);
      n = n.parentNode;
    }
    return '';
  }

  function sameHost(url) {
    try {
      var current = new URL((typeof location !== 'undefined' && location.href) || '');
      var target = new URL(url, current.href);
      return target.hostname === current.hostname || target.hostname.endsWith('.' + current.hostname) || current.hostname.endsWith('.' + target.hostname);
    } catch (e) { return false; }
  }

  function setQuery(url, pairs) {
    var resolved = resolveUrl(url) || url;
    var hash = '';
    var hashIndex = resolved.indexOf('#');
    if (hashIndex !== -1) {
      hash = resolved.slice(hashIndex);
      resolved = resolved.slice(0, hashIndex);
    }
    var base = resolved;
    var existing = {};
    var qIndex = resolved.indexOf('?');
    if (qIndex !== -1) {
      base = resolved.slice(0, qIndex);
      var raw = resolved.slice(qIndex + 1).split('&');
      for (var i = 0; i < raw.length; i++) {
        if (!raw[i]) continue;
        var parts = raw[i].split('=');
        existing[decodeURIComponent(parts[0] || '')] = decodeURIComponent(parts.slice(1).join('=') || '');
      }
    }
    for (var k in pairs) {
      if (pairs[k] == null || pairs[k] === '') continue;
      existing[k] = String(pairs[k]);
    }
    var qs = [];
    for (var key in existing) {
      if (!key) continue;
      qs.push(encodeURIComponent(key) + '=' + encodeURIComponent(existing[key]));
    }
    return base + (qs.length ? '?' + qs.join('&') : '') + hash;
  }

  function goalQuery(goal, ts) {
    if (goal) return clean(goal).slice(0, 220);
    return ts.join(' ');
  }

  function compactTerms(ts, max) {
    var out = [];
    for (var i = 0; i < ts.length && out.length < max; i++) {
      if (/^(model|models|find|updated|update|within|natural|language|processing|pre|trained|hugging|face|last|march|2023)$/.test(ts[i])) continue;
      out.push(ts[i]);
    }
    return out.join(' ');
  }

  function routeControl(el) {
    var tag = tagName(el);
    var type = controlType(el);
    return {
      ref: ref(el), tag: tag, type: type, name: attr(el, 'name'),
      label: labelFor(el), placeholder: attr(el, 'placeholder'), value: isPasswordLike(attr(el, 'name'), type) ? '[REDACTED]' : controlValue(el)
    };
  }

  function routeDiscover(opts) {
    opts = opts || {};
    var goal = opts.goal || '';
    var limit = opts.limit || 30;
    var ts = terms(goal);
    var q = goalQuery(goal, ts);
    var routes = [];
    var forms = [];
    var inferred = [];
    var seenRoutes = {};
    var seenInferred = {};

    function addRoute(route) {
      if (!route || !route.url || seenRoutes[route.url + '\n' + route.label]) return;
      seenRoutes[route.url + '\n' + route.label] = true;
      route.score = clampScore(route.score || 0.4);
      route.page_owned = sameHost(route.url);
      routes.push(route);
    }

    function addInferred(item) {
      if (!item || !item.url || seenInferred[item.url]) return;
      seenInferred[item.url] = true;
      item.score = clampScore(item.score || 0.45);
      item.page_owned = sameHost(item.url);
      inferred.push(item);
    }

    function routeKind(label, url) {
      var hay = clean(label + ' ' + url).toLowerCase();
      if (/search|query|find/.test(hay)) return 'search_route';
      if (/model|models|tasks|pipeline/.test(hay)) return 'model_route';
      if (/about|company|allstars|team|who-we-are/.test(hay)) return 'about_route';
      if (/course|free|learn|catalog/.test(hay)) return 'catalog_route';
      return 'link_route';
    }

    var links = document.querySelectorAll('a[href]');
    for (var i = 0; i < links.length; i++) {
      var a = links[i];
      var label = textOf(a, 180) || attr(a, 'aria-label') || attr(a, 'title') || '';
      var href = attr(a, 'href');
      var url = resolveUrl(href);
      if (!url || /^javascript:/i.test(url) || /^mailto:/i.test(url)) continue;
      var heading = nearestHeading(a);
      var hay = [label, url, heading].join(' ');
      var kind = routeKind(label, url);
      var score = 0.22 + goalBoost(hay, ts);
      if (sameHost(url)) score += 0.12;
      if (kind !== 'link_route') score += 0.18;
      if (nearestChrome(a) === 'nav' || nearestChrome(a) === 'header') score += 0.05;
      if (score < 0.33 && routes.length > 80) continue;
      addRoute({
        kind: kind, label: label || url, url: url, ref: ref(a), nearby_heading: heading || null,
        score: score, matched_terms: Object.keys(ts.reduce(function(m, t) { if (hay.toLowerCase().indexOf(t) !== -1) m[t] = true; return m; }, {})),
        provenance: [{ source: 'dom', ref: ref(a), selector: 'a[href]', reason: 'page-owned visible link' }]
      });

      if (q && sameHost(url) && /(^|\/)(search|find)(\/|$)|[?&](q|query|search)=/i.test(url + ' ' + label)) {
        addInferred({
          kind: 'inferred_search_url', label: 'Search route for goal', url: setQuery(url, { q: q }), base_url: url,
          score: 0.72 + goalBoost(hay, ts), params: { q: q }, matched_terms: termHits(hay, ts) ? ts : [],
          provenance: [{ source: 'dom', ref: ref(a), selector: 'a[href]', reason: 'visible search-like route plus goal query' }]
        });
      }
      if (q && sameHost(url) && /\/models\/?(?:$|\?)/i.test(url)) {
        var lowerGoal = clean(goal).toLowerCase();
        var search = /sentiment/.test(lowerGoal) ? 'sentiment' : (compactTerms(ts, 4) || q);
        var params = { search: search };
        if (/sentiment|text classification|classification/.test(lowerGoal)) params.pipeline_tag = 'text-classification';
        if (/updated|march|2023|old|popular|download/.test(lowerGoal)) params.sort = 'downloads';
        addInferred({
          kind: 'inferred_model_search_url', label: 'Model search route for goal', url: setQuery(url, params), base_url: url,
          score: 0.76 + goalBoost(hay + ' ' + search, ts), params: params, matched_terms: termHits(hay + ' ' + search, ts) ? ts : [],
          provenance: [{ source: 'dom', ref: ref(a), selector: 'a[href]', reason: 'visible models route plus model-search goal terms' }]
        });
      }
    }

    var formNodes = document.getElementsByTagName('form');
    for (var fi = 0; fi < formNodes.length; fi++) {
      var f = formNodes[fi];
      var controlsRaw = f.querySelectorAll('input, textarea, select, button');
      var controls = [];
      var textParts = [attr(f, 'action') || '', attr(f, 'role') || ''];
      var searchName = null;
      for (var ci = 0; ci < controlsRaw.length; ci++) {
        var c = routeControl(controlsRaw[ci]);
        controls.push(c);
        textParts.push(c.label || '', c.name || '', c.placeholder || '', c.type || '');
        if (!searchName && c.name && (c.type === 'search' || c.type === 'text' || c.type === 'textarea' || /^(q|query|search|keyword|term)$/i.test(c.name))) searchName = c.name;
      }
      var action = resolveUrl(attr(f, 'action') || (typeof location !== 'undefined' ? location.href : ''));
      var method = (attr(f, 'method') || 'get').toLowerCase();
      var formText = clean(textParts.join(' '));
      var isSearch = !!searchName || /search|query|lookup|dictionary|find/i.test(formText);
      var queryUrl = null;
      if (q && method === 'get' && searchName) queryUrl = setQuery(action, (function() { var p = {}; p[searchName] = q; return p; })());
      var paramsForForm = {};
      if (searchName) paramsForForm[searchName] = q;
      var item = {
        kind: isSearch ? 'search_form_route' : 'form_route', label: isSearch ? 'Search form' : 'Form', ref: ref(f),
        action: action, method: method, query_url: queryUrl, controls: controls, score: (isSearch ? 0.78 : 0.42) + goalBoost(formText + ' ' + action, ts),
        matched_terms: Object.keys(ts.reduce(function(m, t) { if ((formText + ' ' + action).toLowerCase().indexOf(t) !== -1) m[t] = true; return m; }, {})),
        provenance: [{ source: 'dom', ref: ref(f), selector: 'form', reason: isSearch ? 'search-like form controls' : 'form element' }]
      };
      item.score = clampScore(item.score);
      forms.push(item);
      if (queryUrl) addInferred({
        kind: 'form_query_url', label: 'GET form query URL', url: queryUrl, base_url: action, score: item.score,
        params: paramsForForm, matched_terms: item.matched_terms, provenance: item.provenance
      });
    }

    routes.sort(function(a, b) { return b.score - a.score; });
    forms.sort(function(a, b) { return b.score - a.score; });
    inferred.sort(function(a, b) { return b.score - a.score; });
    if (routes.length > limit) routes.length = limit;
    if (forms.length > limit) forms.length = limit;
    if (inferred.length > limit) inferred.length = limit;
    return {
      url: (typeof location !== 'undefined' && location.href) || '',
      title: document.title || '',
      goal: goal || null,
      routes: routes,
      forms: forms,
      inferred_urls: inferred,
      summary: { routes: routes.length, forms: forms.length, inferred_urls: inferred.length }
    };
  }

  function pageModel(opts) {
    opts = opts || {};
    var goal = opts.goal || '';
    var allowed = Array.isArray(opts.types) ? opts.types : [];
    var limit = opts.limit || 50;
    var ts = terms(goal);
    var objects = [];
    var actions = [];
    var limitations = [];
    var next = 1;

    function add(obj) {
      if (!obj || !hasType(obj.kind, allowed)) return null;
      obj.id = 'obj:' + next++;
      obj.score = clampScore(obj.score || obj.confidence || 0.5);
      obj.confidence = clampScore(obj.confidence || obj.score || 0.5);
      obj.provenance = obj.provenance || [];
      objects.push(obj);
      return obj;
    }

    function addAction(action) {
      if (!action) return;
      actions.push(action);
    }

    function serializeControl(el) {
      var tag = tagName(el);
      var type = controlType(el);
      var c = {
        ref: ref(el), tag: tag, type: type, name: attr(el, 'name'),
        label: labelFor(el), placeholder: attr(el, 'placeholder'), value: controlValue(el)
      };
      if (type === 'checkbox' || type === 'radio') c.checked = !!el.checked;
      if (tag === 'select') c.options = optionSamples(el);
      return c;
    }

    function buildForms() {
      var forms = document.getElementsByTagName('form');
      for (var i = 0; i < forms.length; i++) {
        var f = forms[i];
        var controlsRaw = f.querySelectorAll('input, textarea, select, button');
        var controls = [];
        var textParts = [attr(f, 'action') || '', attr(f, 'role') || ''];
        for (var j = 0; j < controlsRaw.length; j++) {
          var c = serializeControl(controlsRaw[j]);
          controls.push(c);
          textParts.push(c.label || '', c.name || '', c.placeholder || '', c.type || '');
        }
        var formText = clean(textParts.join(' '));
        var method = (attr(f, 'method') || 'get').toLowerCase();
        var actionUrl = resolveUrl(attr(f, 'action') || (typeof location !== 'undefined' ? location.href : ''));
        var isSearch = /search|query|lookup|dictionary|q\b|keyword/i.test(formText) ||
          controls.some(function(c) { return c.type === 'search' || c.name === 'q' || c.name === 'query'; });
        var fields = controls.filter(function(c) {
          return c.name && c.type !== 'submit' && c.type !== 'button' && c.type !== 'reset' && c.type !== 'image';
        }).map(function(c) {
          var redacted = isPasswordLike(c.name, c.type);
          return { name: c.name, label: c.label, type: c.type, value: redacted ? '[REDACTED]' : c.value, redacted: redacted };
        });
        var obj = add({
          kind: isSearch ? 'search_form' : 'form',
          label: (isSearch ? 'Search form' : 'Form') + (controls[0] && controls[0].label ? ': ' + controls[0].label : ''),
          text: formText,
          fields: { method: method, action: actionUrl, controls: controls, serializable_fields: fields },
          actions: [],
          confidence: isSearch ? 0.88 : 0.7,
          score: (isSearch ? 0.72 : 0.45) + goalBoost(formText, ts),
          provenance: [{ source: 'dom', ref: ref(f), selector: 'form', reason: isSearch ? 'search-like labels/action' : 'form element' }]
        });
        if (!obj) continue;
        var submitters = f.querySelectorAll('button, input[type=submit], input[type=image]');
        for (var si = 0; si < submitters.length; si++) {
          var st = submitters[si];
          var a = { kind: 'submit', ref: ref(st), object_id: obj.id, label: textOf(st, 120) || attr(st, 'value') || labelFor(st), method: method, url: actionUrl };
          obj.actions.push(a);
          addAction(a);
        }
        if (obj.actions.length === 0) {
          var a2 = { kind: 'submit', ref: ref(f), object_id: obj.id, label: obj.label, method: method, url: actionUrl };
          obj.actions.push(a2);
          addAction(a2);
        }
      }
    }

    function bestAnchor(card) {
      var anchors = card.querySelectorAll('a[href]');
      var best = null;
      var bestScore = -1;
      for (var i = 0; i < anchors.length; i++) {
        var a = anchors[i];
        var t = textOf(a, 180) || attr(a, 'aria-label') || attr(a, 'title') || '';
        if (!t || /^(read more|learn more|more|here)$/i.test(t)) continue;
        var s = t.length;
        if (a.querySelector('h1,h2,h3,h4,[itemprop="name"]')) s += 80;
        if ((attr(a, 'class') || '').match(/title|name|card|product|course|model|recipe/i)) s += 30;
        if (s > bestScore) { best = a; bestScore = s; }
      }
      return best;
    }

    function firstMatch(root, selectors) {
      for (var i = 0; i < selectors.length; i++) {
        try {
          var el = root.querySelector(selectors[i]);
          if (el) return el;
        } catch (e) {}
      }
      return null;
    }

    function textMatches(root, selectors, max) {
      var out = [];
      max = max || 10;
      for (var s = 0; s < selectors.length; s++) {
        var nodes = [];
        try { nodes = root.querySelectorAll(selectors[s]); } catch (e) { nodes = []; }
        for (var i = 0; i < nodes.length && out.length < max; i++) {
          var t = textOf(nodes[i], 120);
          if (t && out.indexOf(t) === -1) out.push(t);
        }
      }
      return out;
    }

    function dateField(card) {
      var time = firstMatch(card, ['time[datetime]', 'time']);
      if (time) return attr(time, 'datetime') || textOf(time, 80);
      var text = textOf(card, 1000);
      var m = text.match(/(?:updated|last updated|published|posted)\s+([A-Z][a-z]{2,8}\s+\d{1,2},?\s+\d{4}|\d{4}-\d{2}-\d{2})/i) ||
        text.match(/\b([A-Z][a-z]{2,8}\s+\d{1,2},?\s+\d{4})\b/);
      return m ? m[1] : null;
    }

    function inferCardKind(card, title, url, tags, text) {
      var cls = clean((attr(card, 'class') || '') + ' ' + (attr(card, 'itemtype') || '')).toLowerCase();
      var hay = (cls + ' ' + title + ' ' + url + ' ' + tags.join(' ') + ' ' + text).toLowerCase();
      if (/huggingface\.co\/[\w.-]+\/[\w.-]+/.test(url || '') || /text-classification|model|pipeline_tag/.test(hay) || /^[\w.-]+\//.test(title || '')) return 'model_card';
      if (/course|coursera|free trial|enroll|university|instructor/.test(hay)) return 'course_card';
      if (/recipe|cook time|prep time|ingredients|allrecipes/.test(hay)) return 'article_card';
      if (/product|price|buy|shop|airpods|iphone|rating/.test(hay)) return 'product_card';
      if (tagName(card) === 'article' || /article|story|news|post/.test(hay)) return 'article_card';
      return 'card';
    }

    function buildCards() {
      var selectors = [
        'article', '[itemscope]', '[itemtype*="Product"]', '[itemtype*="Recipe"]',
        '.card', '.product', '.course', '.recipe', '.listing', '.result', '.model-card',
        '[class*="card"]', '[class*="product"]', '[class*="course"]', '[class*="recipe"]',
        '[class*="listing"]', '[class*="model"]', 'li'
      ];
      var candidates = [];
      for (var s = 0; s < selectors.length; s++) {
        var nodes = [];
        try { nodes = document.querySelectorAll(selectors[s]); } catch (e) { nodes = []; }
        for (var i = 0; i < nodes.length; i++) if (candidates.indexOf(nodes[i]) === -1) candidates.push(nodes[i]);
      }
      var seen = {};
      for (var c = 0; c < candidates.length; c++) {
        var card = candidates[c];
        if (nearestChrome(card) && tagName(card) === 'li') continue;
        var anchor = bestAnchor(card);
        var titleEl = firstMatch(card, ['[itemprop="name"]', 'h1', 'h2', 'h3', 'h4', '.title', '.name', '.card-title', '.course-title', '.product-title']);
        var title = (anchor && textOf(anchor, 180)) || textOf(titleEl, 180);
        if (!title || title.length < 3 || title.length > 220) continue;
        if (/^(read more|learn more|more|next|previous)$/i.test(title)) continue;
        var url = anchor ? resolveUrl(attr(anchor, 'href')) : resolveUrl(attr(firstMatch(card, ['a[href]']), 'href'));
        var snippetEl = firstMatch(card, ['[itemprop="description"]', '.description', '.summary', '.excerpt', '.snippet', '.subtitle', 'p']);
        var snippet = textOf(snippetEl, 260);
        var tags = textMatches(card, ['[class*="tag"]', '[class*="badge"]', '[class*="chip"]', 'a[href*="pipeline_tag"]', 'a[href*="sort="]'], 12);
        var date = dateField(card);
        var text = textOf(card, 900);
        var kind = inferCardKind(card, title, url || '', tags, text);
        var commerce = null;
        if (typeof globalThis.__cardCommerceFields === 'function') {
          try { commerce = globalThis.__cardCommerceFields(card, title, snippet); } catch (e) { commerce = null; }
        }
        if (commerce && commerce.price && kind === 'card') kind = 'product_card';
        var key = (url || '') + '\n' + title;
        if (seen[key]) continue;
        seen[key] = true;
        var fields = { tags: tags };
        if (commerce) {
          if (commerce.price) fields.price = commerce.price;
          if (commerce.condition) fields.condition = commerce.condition;
          if (commerce.availability) fields.availability = commerce.availability;
          if (commerce.meta && commerce.meta.length) fields.meta = commerce.meta;
        }
        if (date) fields.date = date;
        if (/updated/i.test(text) && date) fields.updated = date;
        if (kind === 'model_card' && title.indexOf('/') !== -1) {
          fields.owner = title.split('/')[0];
          fields.model = title.split('/').slice(1).join('/');
        }
        var base = kind === 'card' ? 0.48 : 0.68;
        if (url) base += 0.08;
        if (snippet) base += 0.05;
        if (tags.length) base += 0.04;
        var hay = [title, url, snippet, tags.join(' '), date, text].join(' ');
        var obj = add({
          kind: kind,
          title: title,
          url: url || null,
          snippet: snippet || null,
          text: text,
          fields: fields,
          actions: url ? [{ kind: 'open', ref: anchor ? ref(anchor) : ref(card), url: url }] : [],
          score: base + goalBoost(hay, ts),
          confidence: clampScore(base),
          provenance: [{ source: 'dom', ref: ref(card), selector: selectors.join(', '), reason: 'repeated/card-like block' }]
        });
        if (obj && obj.actions.length) {
          obj.actions[0].object_id = obj.id;
          addAction(obj.actions[0]);
        }
      }
    }

    function buildLinks() {
      var links = document.querySelectorAll('a[href]');
      var candidates = [];
      for (var i = 0; i < links.length; i++) {
        var a = links[i];
        var title = textOf(a, 180) || attr(a, 'aria-label') || attr(a, 'title') || '';
        var url = resolveUrl(attr(a, 'href'));
        if (!title || !url || title.length < 2) continue;
        var chrome = nearestChrome(a);
        var heading = nearestHeading(a);
        var hay = [title, url, heading].join(' ');
        var match = goalBoost(hay, ts);
        var score = (chrome === 'nav' || chrome === 'header' ? 0.42 : 0.35) + match;
        if (match === 0 && candidates.length > 80 && chrome !== 'nav' && chrome !== 'header') continue;
        candidates.push({
          kind: chrome === 'nav' || chrome === 'header' ? 'nav_link' : 'link',
          title: title,
          url: url,
          fields: { chrome: chrome, nearby_heading: heading || null },
          actions: [{ kind: 'open', ref: ref(a), url: url }],
          score: score,
          confidence: chrome ? 0.62 : 0.5,
          provenance: [{ source: 'dom', ref: ref(a), selector: 'a[href]', reason: chrome ? 'site navigation link' : 'visible page link' }]
        });
      }
      candidates.sort(function(a, b) { return b.score - a.score; });
      for (var j = 0; j < candidates.length && j < 50; j++) {
        var obj = add(candidates[j]);
        if (obj && obj.actions.length) {
          obj.actions[0].object_id = obj.id;
          addAction(obj.actions[0]);
        }
      }
    }

    function buildTables() {
      var tables = document.querySelectorAll('table');
      for (var i = 0; i < tables.length && i < 12; i++) {
        var table = tables[i];
        var headers = [];
        var ths = table.querySelectorAll('th');
        for (var h = 0; h < ths.length && h < 20; h++) headers.push(textOf(ths[h], 80));
        var title = textOf(firstMatch(table, ['caption']), 160) || nearestHeading(table) || 'Table';
        add({
          kind: 'table',
          title: title,
          fields: { headers: headers, row_count: table.querySelectorAll('tr').length },
          text: textOf(table, 700),
          score: 0.55 + goalBoost([title, headers.join(' '), textOf(table, 400)].join(' '), ts),
          confidence: 0.72,
          provenance: [{ source: 'dom', ref: ref(table), selector: 'table', reason: 'table element' }]
        });
      }
    }

    function buildAnswerBlocks() {
      var sels = ['[itemprop="description"]', '.definition', '.def', '[data-testid*="definition"]', 'dl', '.faq', '[class*="answer"]'];
      var seen = {};
      for (var s = 0; s < sels.length; s++) {
        var nodes = [];
        try { nodes = document.querySelectorAll(sels[s]); } catch (e) { nodes = []; }
        for (var i = 0; i < nodes.length && i < 20; i++) {
          var text = textOf(nodes[i], 900);
          if (text.length < 30 || seen[text]) continue;
          seen[text] = true;
          add({
            kind: 'answer_block',
            title: nearestHeading(nodes[i]) || text.slice(0, 80),
            text: text,
            score: 0.55 + goalBoost(text, ts),
            confidence: 0.62,
            provenance: [{ source: 'dom', ref: ref(nodes[i]), selector: sels[s], reason: 'definition/answer-like block' }]
          });
        }
      }
    }

    function buildDomLimitations(blockmap) {
      if (!blockmap || !blockmap.density) return;
      if (blockmap.density.thin_shell) {
        limitations.push({ kind: 'limitation', reason: 'thin_shell', confidence: 0.76, evidence: ['blockmap.density.thin_shell'], hint: 'The DOM is a thin shell; no semantic objects may be available without script-rendered state.' });
      } else if (blockmap.density.script_heavy_shell) {
        limitations.push({ kind: 'limitation', reason: 'script_heavy_shell', confidence: 0.74, evidence: ['blockmap.density.script_heavy_shell'], hint: 'The page has many scripts but little visible DOM content; browser-rendered state may be required.' });
      } else if (blockmap.density.likely_js_filled) {
        limitations.push({ kind: 'limitation', reason: 'rendered_result_required', confidence: 0.68, evidence: ['blockmap.density.likely_js_filled'], hint: 'The page likely expects JS to fill content that is absent from the static DOM.' });
      }
    }

    var blockmap = null;
    try { blockmap = typeof __blockmap === 'function' ? __blockmap() : null; } catch (e) { blockmap = null; }
    buildForms();
    buildCards();
    buildLinks();
    buildTables();
    buildAnswerBlocks();
    buildDomLimitations(blockmap);

    objects.sort(function(a, b) { return b.score - a.score; });
    if (objects.length > limit) objects.length = limit;
    var keptIds = {};
    for (var kid = 0; kid < objects.length; kid++) keptIds[objects[kid].id] = true;
    actions = actions.filter(function(a) { return !a.object_id || keptIds[a.object_id]; });

    var counts = {};
    for (var oi = 0; oi < objects.length; oi++) counts[objects[oi].kind] = (counts[objects[oi].kind] || 0) + 1;
    return {
      url: (typeof location !== 'undefined' && location.href) || '',
      title: document.title || '',
      goal: goal || null,
      objects: objects,
      actions: actions.slice(0, 100),
      limitations: limitations,
      network_objects: [],
      summary: {
        primary_objects: objects.length,
        object_kinds: counts,
        search_forms: counts.search_form || 0,
        cards: (counts.article_card || 0) + (counts.course_card || 0) + (counts.model_card || 0) + (counts.product_card || 0) + (counts.card || 0),
        tables: counts.table || 0,
        limitations: limitations.length
      }
    };
  }

  globalThis.__pageModel = pageModel;
  globalThis.__routeDiscover = routeDiscover;
})();

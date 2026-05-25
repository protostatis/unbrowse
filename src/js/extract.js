// Auto-strategy extraction. Tries a fixed sequence of public-knowledge
// strategies, scores each by data richness, returns the best one. No
// private dependency — all strategies use only standard DOM/JSON shapes
// the open web uses to expose structured data.
//
// Strategy order is roughly "highest signal-to-noise first":
//   1. json_ld    — <script type="application/ld+json"> (Schema.org)
//   2. next_data  — Next.js __NEXT_DATA__ JSON blob
//   3. nuxt_data  — Nuxt __NUXT__ blob
//   4. og_meta    — OpenGraph + Twitter Card + standard meta tags
//   5. microdata  — itemscope/itemprop walk (Schema.org-in-HTML)
//   6. text_main  — chrome-stripped main content (always available as fallback)
//
// Each returns { strategy, confidence, data, hint? } or null. Confidence is
// a rough 0..1 — JSON-LD with Article > 0.9, OG with title+description ~0.6,
// text_main fallback ~0.3.

(function () {
  function safeJSONParse(s) {
    try { return JSON.parse(s); } catch { return null; }
  }

  function strategyJsonLd() {
    var nodes = document.querySelectorAll('script[type="application/ld+json"]');
    if (!nodes.length) return null;
    var blobs = [];
    for (var i = 0; i < nodes.length; i++) {
      var raw = nodes[i].textContent || '';
      if (!raw.trim()) continue;
      var parsed = safeJSONParse(raw);
      if (parsed) blobs.push(parsed);
    }
    if (!blobs.length) return null;
    // Single object → return directly. Array of @graph entries → flatten.
    var data = blobs.length === 1 ? blobs[0] : blobs;
    // Confidence: if any blob has @type set to a real schema (Article, Product,
    // Recipe, etc.), it's high-signal. Otherwise medium.
    var hasType = blobs.some(function (b) {
      if (b && b['@type']) return true;
      if (b && b['@graph'] && Array.isArray(b['@graph'])) {
        return b['@graph'].some(function (g) { return g && g['@type']; });
      }
      return false;
    });
    return {
      strategy: 'json_ld',
      confidence: hasType ? 0.95 : 0.7,
      data: data,
    };
  }

  function strategyNextData() {
    var el = document.querySelector('script#__NEXT_DATA__');
    if (!el) return null;
    var raw = el.textContent || '';
    var parsed = safeJSONParse(raw);
    if (!parsed) return null;
    // Drill to the most useful subtree if available.
    var page = parsed && parsed.props && parsed.props.pageProps;
    if (!page) {
      return { strategy: 'next_data', confidence: 0.7, data: parsed };
    }
    // pageProps with substantial nested content is almost always the page's
    // primary app data (markets list on Polymarket, products on a Shopify-
    // Next site, posts on a forum). Bump above json_ld's 0.95 so the
    // auto-picker prefers it over the typically-smaller schema.org metadata.
    // Threshold (1 KB serialized) excludes routing-only pageProps that some
    // marketing pages emit.
    var size = 0;
    try { size = JSON.stringify(page).length; } catch (e) {}
    return {
      strategy: 'next_data',
      confidence: size > 1024 ? 0.97 : 0.85,
      data: page,
    };
  }

  // Next.js App Router (RSC) — the new replacement for __NEXT_DATA__. The
  // page emits many <script> blocks of the form
  //   self.__next_f.push([1, "<key>:<value>\n<key>:<value>\n..."])
  // where each line is a Flight-protocol entry. We concatenate the string
  // payloads (chunk-boundary-agnostic by design — that's the protocol),
  // split into <key>:<value> records, and JSON-parse the values that are
  // plain data (skipping the "$S..." type-symbol lines and "I[id,...]"
  // module-reference lines that aren't content).
  //
  // Returns the parsed entries as a {key: value} dict. The key is the
  // chunk's reference id; values are the route/page state, server-action
  // returns, and prefetched data the page would otherwise hydrate from.
  function strategyRscPayload() {
    var pushRe = /__next_f\.push\(\s*\[\s*1\s*,\s*("(?:\\.|[^"\\])*")\s*\]\s*\)/g;
    var chunks = [];
    // First try the live DOM (works on sites that don't clean up after
    // hydration — e.g. Tailwind).
    var scripts = document.querySelectorAll('script');
    for (var i = 0; i < scripts.length; i++) {
      var t = scripts[i].textContent || '';
      if (t.indexOf('__next_f.push') < 0) continue;
      pushRe.lastIndex = 0;
      var m;
      while ((m = pushRe.exec(t)) !== null) {
        try { chunks.push(JSON.parse(m[1])); } catch (e) {}
      }
    }
    // Fallback: scan the raw HTML body if hydration scripts have removed
    // the inline `<script>` blocks (Next.js App Router does this — by the
    // time we extract, the live DOM has 0 scripts containing __next_f even
    // though the original body had dozens). The host function returns the
    // body of the most recent navigate.
    if (!chunks.length && typeof __host_raw_body === 'function') {
      var raw = __host_raw_body();
      if (raw) {
        pushRe.lastIndex = 0;
        var m2;
        while ((m2 = pushRe.exec(raw)) !== null) {
          try { chunks.push(JSON.parse(m2[1])); } catch (e) {}
        }
      }
    }
    if (!chunks.length) return null;

    var combined = chunks.join('');
    var entries = {};
    var lines = combined.split('\n');
    for (var j = 0; j < lines.length; j++) {
      var line = lines[j];
      if (!line) continue;
      var colon = line.indexOf(':');
      if (colon <= 0) continue;
      var key = line.substring(0, colon);
      var value = line.substring(colon + 1);
      if (!key || !value) continue;
      // Skip module references and type symbols — they're plumbing, not data:
      //   "$Sreact.fragment" — symbol reference
      //   I[id, [chunks], "name"] — client component module reference
      //   HL[...] — hint/preload directive
      var c0 = value.charAt(0);
      if (c0 === 'I' && value.charAt(1) === '[') continue;
      if (c0 === 'H' && (value.charAt(1) === 'L' || value.charAt(1) === 'B')) continue;
      if (c0 === '"' && value.charAt(1) === '$') continue;
      try {
        entries[key] = JSON.parse(value);
      } catch (e) { /* not parseable JSON — skip */ }
    }
    var keyCount = Object.keys(entries).length;
    if (keyCount === 0) return null;
    var size = 0;
    try { size = JSON.stringify(entries).length; } catch (e) {}
    // Confidence parallels next_data: substantial RSC data is the page's
    // primary state. Below next_data's 0.97 so a hybrid page (rare) prefers
    // pageProps. Above json_ld so SPAs without metadata still surface data.
    return {
      strategy: 'rsc_payload',
      confidence: size > 1024 ? 0.93 : 0.75,
      data: entries,
    };
  }

  function strategyNuxtData() {
    // Nuxt drops a global window.__NUXT__ that's a JS object literal. We
    // can't trivially read it without exec_scripts:true; check both the
    // raw script form (Nuxt also embeds it in a <script id=__NUXT_DATA__>
    // since Nuxt 3) and the runtime global.
    var el = document.querySelector('script#__NUXT_DATA__');
    if (el) {
      var raw = el.textContent || '';
      var parsed = safeJSONParse(raw);
      if (parsed) return { strategy: 'nuxt_data', confidence: 0.85, data: parsed };
    }
    if (typeof window !== 'undefined' && window.__NUXT__) {
      return { strategy: 'nuxt_data', confidence: 0.85, data: window.__NUXT__ };
    }
    return null;
  }

  function strategyOgMeta() {
    var metas = document.querySelectorAll('meta');
    if (!metas.length) return null;
    var out = {};
    var keys = 0;
    for (var i = 0; i < metas.length; i++) {
      var m = metas[i];
      var k = m.getAttribute('property') || m.getAttribute('name') || '';
      var v = m.getAttribute('content') || '';
      if (!k || !v) continue;
      // Keep only the high-signal namespaces.
      if (k.indexOf('og:') === 0 || k.indexOf('twitter:') === 0 ||
          k === 'description' || k === 'keywords' || k === 'author' ||
          k === 'article:published_time' || k === 'article:author') {
        out[k] = v;
        keys++;
      }
    }
    if (!keys) return null;
    var titleEl = document.querySelector('title');
    if (titleEl) out['_title'] = (titleEl.textContent || '').trim();
    var canonical = document.querySelector('link[rel=canonical]');
    if (canonical) out['_canonical'] = canonical.getAttribute('href');
    // Confidence scales with how many of the core fields are present.
    var hasTitle = out['og:title'] || out['twitter:title'] || out['_title'];
    var hasDesc = out['og:description'] || out['twitter:description'] || out['description'];
    var conf = 0.4;
    if (hasTitle && hasDesc) conf = 0.65;
    if (out['og:type'] === 'article' || out['og:type'] === 'product') conf = 0.75;
    return { strategy: 'og_meta', confidence: conf, data: out };
  }

  function strategyMicrodata() {
    var roots = document.querySelectorAll('[itemscope]');
    if (!roots.length) return null;
    function readItem(el) {
      var item = {};
      var typeAttr = el.getAttribute('itemtype');
      if (typeAttr) item['@type'] = typeAttr;
      // Walk descendants looking for itemprop, but stop descending when we
      // hit another itemscope (that's a nested item, captured separately).
      var stack = [].concat(el.childNodes || []);
      while (stack.length) {
        var node = stack.shift();
        if (!node || node.nodeType !== 1) continue;
        var prop = node.getAttribute('itemprop');
        if (prop) {
          var v;
          if (node.hasAttribute('itemscope')) {
            v = readItem(node);
          } else {
            var tag = (node.tagName || '').toLowerCase();
            v = node.getAttribute('content') || node.getAttribute('href') ||
                node.getAttribute('src') || node.getAttribute('datetime') ||
                (tag === 'meta' ? node.getAttribute('content') : '') ||
                (node.textContent || '').trim();
          }
          if (item[prop] === undefined) item[prop] = v;
          else if (Array.isArray(item[prop])) item[prop].push(v);
          else item[prop] = [item[prop], v];
        }
        if (!node.hasAttribute('itemscope')) {
          for (var i = 0; i < (node.childNodes || []).length; i++) {
            stack.push(node.childNodes[i]);
          }
        }
      }
      return item;
    }
    var items = [];
    for (var r = 0; r < roots.length; r++) {
      var root = roots[r];
      // Skip nested itemscopes (they'll be captured by their parent).
      var p = root.parentNode;
      var nested = false;
      while (p && p.nodeType === 1) {
        if (p.hasAttribute && p.hasAttribute('itemscope')) { nested = true; break; }
        p = p.parentNode;
      }
      if (!nested) items.push(readItem(root));
    }
    if (!items.length) return null;
    return {
      strategy: 'microdata',
      confidence: items.length > 1 ? 0.7 : 0.6,
      data: items.length === 1 ? items[0] : items,
    };
  }

  // Magento, Shopify, BigCommerce, et al. embed product/page data in
  // custom-typed <script> tags so their own client JS can consume it.
  // Common shapes:
  //   <script type="text/x-magento-init">{...}</script>     (Magento, often dozens per page)
  //   <script type="text/x-shopify-app">{...}</script>      (Shopify)
  //   <script type="application/vnd.shopify.product+json">  (newer Shopify)
  //   <script id="bc-product">{...}</script>                (BigCommerce)
  //
  // Generalized: any <script> whose `type` is not a JS variant AND whose
  // textContent parses as JSON. We collect them keyed by type, returning
  // a flat object the agent can iterate. This catches the SSR-but-
  // products-in-script class of pages that look "static" (full nav chrome
  // + filter UI + headings) but whose actual data lives in script tags.
  function strategyJsonInScript() {
    var scripts = document.querySelectorAll('script[type]');
    if (!scripts.length) return null;
    var collected = {}; // type -> [parsed blobs]
    var hits = 0;
    for (var i = 0; i < scripts.length; i++) {
      var s = scripts[i];
      var t = (s.getAttribute('type') || '').toLowerCase();
      // Skip pure JS — already-recognized JSON shapes are handled by
      // dedicated strategies (json_ld, next_data, nuxt_data, og_meta).
      if (!t || t === 'text/javascript' || t === 'module' ||
          t === 'application/javascript' ||
          t === 'application/ld+json') continue;
      // Only consider types that strongly imply JSON payload.
      var looksJson = t.indexOf('json') !== -1 ||
                      t.indexOf('x-magento') !== -1 ||
                      t.indexOf('x-shopify') !== -1 ||
                      t.indexOf('x-component') !== -1;
      if (!looksJson) continue;
      var raw = (s.textContent || '').trim();
      if (!raw || raw[0] !== '{' && raw[0] !== '[') continue;
      var parsed = safeJSONParse(raw);
      if (!parsed) continue;
      if (!collected[t]) collected[t] = [];
      collected[t].push(parsed);
      hits++;
    }
    if (!hits) return null;
    // Confidence rises with how many script types we picked up; a single
    // type with one blob is moderate, multiple types or many blobs is
    // high (signals a real SSR-with-JSON-config page like Magento).
    var typeCount = Object.keys(collected).length;
    var conf = typeCount > 1 ? 0.85 : (hits > 5 ? 0.75 : 0.6);
    return {
      strategy: 'json_in_script',
      confidence: conf,
      data: collected,
      hint: hits + ' JSON-bearing script(s) across ' + typeCount + ' type(s)',
    };
  }

  function strategyTextMain() {
    // Always last-resort. The Rust side already exposes text_main via RPC,
    // but we duplicate a thin version here so the extract pipeline can run
    // self-contained. Returns null if nothing meaningful.
    if (typeof __textMain === 'function') {
      var t = __textMain();
      if (t && t.length > 50) {
        return { strategy: 'text_main', confidence: 0.3, data: t };
      }
    }
    var body = document.body ? (document.body.textContent || '').trim() : '';
    if (body.length > 50) {
      return { strategy: 'text_main', confidence: 0.2, data: body };
    }
    return null;
  }

  var SKIP_TEXT_TAGS = {
    script: true, style: true, noscript: true, template: true, svg: true
  };
  var CHROME_TEXT_TAGS = { nav: true, header: true, footer: true, aside: true };

  function collapseWhitespace(s) {
    return String(s || '').replace(/\s+/g, ' ').trim();
  }

  function tagName(node) {
    return ((node && node.tagName) || '').toLowerCase();
  }

  function isHidden(el) {
    if (!el || el.nodeType !== 1) return false;
    if (el.hasAttribute('hidden')) return true;
    if ((el.getAttribute('aria-hidden') || '').toLowerCase() === 'true') return true;
    var style = (el.getAttribute('style') || '').toLowerCase();
    return style.indexOf('display:none') !== -1 ||
           style.indexOf('visibility:hidden') !== -1;
  }

  function looksLikeImageArtifact(s) {
    s = collapseWhitespace(s);
    if (!s) return true;
    if (/^(image|photo|picture|thumbnail|avatar|logo|icon)$/i.test(s)) return true;
    if (/^(data:image\/|https?:\/\/|\/\/)/i.test(s)) return true;
    if (/\.(jpe?g|png|gif|webp|svg|avif)(\?.*)?$/i.test(s)) return true;
    if (/^(src|srcset|alt)=/i.test(s)) return true;
    return false;
  }

  function cleanNodeText(el) {
    var chunks = [];
    function walk(node) {
      if (!node) return;
      if (node.nodeType === 3) {
        var t = collapseWhitespace(node.textContent || '');
        if (t && !looksLikeImageArtifact(t)) chunks.push(t);
        return;
      }
      if (node.nodeType !== 1 && node.nodeType !== 9 && node.nodeType !== 11) return;
      var tag = tagName(node);
      if (SKIP_TEXT_TAGS[tag] || isHidden(node)) return;
      var kids = node.childNodes || [];
      for (var i = 0; i < kids.length; i++) walk(kids[i]);
    }
    walk(el);

    var deduped = [];
    for (var i = 0; i < chunks.length; i++) {
      var chunk = chunks[i];
      if (!chunk) continue;
      if (deduped.length && deduped[deduped.length - 1] === chunk) continue;
      deduped.push(chunk);
    }
    var text = collapseWhitespace(deduped.join(' '));
    var doubled = text.match(/^(.{8,160})\s+\1$/);
    if (doubled) text = doubled[1];
    return text;
  }

  function cleanText(value) {
    if (value && typeof value === 'object' && value.nodeType) {
      return cleanNodeText(value);
    }
    return collapseWhitespace(value);
  }

  function isSkipTag(tag, dropChrome) {
    if (SKIP_TEXT_TAGS[tag]) return true;
    return dropChrome && CHROME_TEXT_TAGS[tag];
  }

  function looksJsonText(s) {
    s = cleanText(s);
    if (s.length < 40) return false;
    var first = s.charAt(0), last = s.charAt(s.length - 1);
    if (!((first === '{' && last === '}') || (first === '[' && last === ']'))) return false;
    var punctuation = (s.match(/[{}[\]":,]/g) || []).length;
    return punctuation / s.length > 0.08;
  }

  function attrsOf(el) {
    return el && el._attributes ? el._attributes : {};
  }

  function textFrom(root, opts) {
    opts = opts || {};
    var out = [];
    var seen = {};
    function walk(node) {
      if (!node) return;
      if (node.nodeType === 3) {
        var t = cleanText(node.textContent || '');
        if (!t || looksJsonText(t)) return;
        if (t.length > 30) {
          if (seen[t]) return;
          seen[t] = true;
        }
        out.push(t);
        return;
      }
      if (node.nodeType !== 1) return;
      var tag = tagName(node);
      if (isSkipTag(tag, opts.dropChrome !== false) || isHidden(node)) return;
      var own = cleanText(node.textContent || '');
      if (looksJsonText(own)) return;
      for (var i = 0; i < (node.childNodes || []).length; i++) walk(node.childNodes[i]);
    }
    walk(root);
    return cleanText(out.join(' '));
  }

  function contentScore(el) {
    if (!el) return -1;
    var tag = tagName(el);
    var text = textFrom(el, { dropChrome: true });
    var score = text.length;
    if (tag === 'article') score += 2000;
    if (tag === 'main') score += 1600;
    if ((el.getAttribute && el.getAttribute('role')) === 'main') score += 1600;
    if (tag === 'section') score += 300;
    return score;
  }

  function bestContentRoot(selector) {
    if (selector) return document.querySelector(selector);
    var candidates = [];
    var sels = ['main', '[role="main"]', 'article', '#content', '#main', '.content', '.article', '.post'];
    for (var s = 0; s < sels.length; s++) {
      var nodes = document.querySelectorAll(sels[s]);
      for (var i = 0; i < nodes.length; i++) candidates.push(nodes[i]);
    }
    var best = null, bestScore = -1;
    for (var j = 0; j < candidates.length; j++) {
      var score = contentScore(candidates[j]);
      if (score > bestScore) { best = candidates[j]; bestScore = score; }
    }
    return best || document.body;
  }

  function limitText(s, maxChars) {
    s = cleanText(s);
    maxChars = maxChars || 0;
    return maxChars > 0 && s.length > maxChars ? s.slice(0, maxChars) : s;
  }

  globalThis.__textClean = function (opts) {
    opts = opts || {};
    var root = bestContentRoot(opts.selector || null);
    if (!root) return '';
    return limitText(textFrom(root, { dropChrome: true }), opts.max_chars || opts.maxChars || 0);
  };

  function matchIndex(haystack, needle, exact) {
    var h = cleanText(haystack);
    var n = cleanText(needle);
    if (!n) return -1;
    if (exact) return h === n ? 0 : -1;
    return h.toLowerCase().indexOf(n.toLowerCase());
  }

  function usefulnessScore(el) {
    var score = 0;
    var p = el;
    while (p && p.nodeType === 1) {
      var tag = tagName(p);
      if (tag === 'article') score += 80;
      else if (tag === 'main') score += 70;
      else if ((p.getAttribute && p.getAttribute('role')) === 'main') score += 70;
      else if (tag === 'section') score += 15;
      else if (CHROME_TEXT_TAGS[tag]) score -= 80;
      p = p.parentNode;
    }
    return score;
  }

  globalThis.__findText = function (opts) {
    opts = opts || {};
    var needle = opts.text || '';
    var exact = !!opts.exact;
    var limit = opts.limit || 20;
    var context = opts.context_chars || opts.contextChars || 80;
    var roots = opts.selector ? document.querySelectorAll(opts.selector) : [document.body];
    var hits = [];
    function visit(node) {
      if (!node || node.nodeType !== 1 || isHidden(node)) return false;
      var tag = tagName(node);
      if (isSkipTag(tag, false)) return false;
      var full = textFrom(node, { dropChrome: false });
      if (matchIndex(full, needle, exact) < 0) return false;
      var childHit = false;
      for (var i = 0; i < (node.childNodes || []).length; i++) {
        if (visit(node.childNodes[i])) childHit = true;
      }
      if (!childHit) {
        var idx = matchIndex(full, needle, exact);
        var m = exact ? full : full.slice(idx, idx + cleanText(needle).length);
        hits.push({
          ref: 'e:' + node._id,
          tag: tag,
          attrs: attrsOf(node),
          before: full.slice(Math.max(0, idx - context), idx),
          match: m,
          after: full.slice(idx + m.length, idx + m.length + context),
          text: full,
          _score: usefulnessScore(node) - Math.min(full.length, 5000) / 10000
        });
      }
      return true;
    }
    for (var r = 0; r < roots.length; r++) visit(roots[r]);
    hits.sort(function (a, b) { return b._score - a._score; });
    if (hits.length > limit) hits.length = limit;
    for (var h = 0; h < hits.length; h++) delete hits[h]._score;
    return hits;
  };

  globalThis.__textAround = function (opts) {
    opts = opts || {};
    var context = opts.context_chars || opts.contextChars || 400;
    var el = null;
    if (opts.ref && typeof __byRef === 'function') el = __byRef(opts.ref);
    if (!el && opts.text) {
      var found = globalThis.__findText({ text: opts.text, selector: opts.selector,
        exact: opts.exact, limit: 1, context_chars: context });
      if (found && found.length && typeof __byRef === 'function') el = __byRef(found[0].ref);
    }
    var root = bestContentRoot(opts.selector || null);
    var full = textFrom(root || document.body, { dropChrome: true });
    var target = el ? textFrom(el, { dropChrome: true }) : cleanText(opts.text || '');
    var idx = target ? full.indexOf(target) : -1;
    if (idx < 0 && opts.text) idx = full.toLowerCase().indexOf(cleanText(opts.text).toLowerCase());
    if (idx < 0) {
      var fallback = target || full;
      return { ref: opts.ref || null, before: '', match: limitText(fallback, context), after: '', text: limitText(fallback, context * 2) };
    }
    var match = target || full.slice(idx, idx + cleanText(opts.text).length);
    return {
      ref: opts.ref || (el ? 'e:' + el._id : null),
      before: full.slice(Math.max(0, idx - context), idx),
      match: match,
      after: full.slice(idx + match.length, idx + match.length + context),
      text: full.slice(Math.max(0, idx - context), idx + match.length + context)
    };
  };

  function firstMatch(root, selectors) {
    for (var i = 0; i < selectors.length; i++) {
      try {
        var el = root.querySelector(selectors[i]);
        if (el) return el;
      } catch (e) {}
    }
    return null;
  }

  function attr(el, name) {
    return el && el.getAttribute ? el.getAttribute(name) : null;
  }

  function resolveHref(href) {
    if (!href) return null;
    if (typeof __host_resolve_url === 'function') {
      try {
        return __host_resolve_url(href, (typeof location !== 'undefined' && location.href) || '') || href;
      } catch (e) {}
    }
    return href;
  }

  function meaningfulTitle(s) {
    s = collapseWhitespace(s);
    if (!s || s.length < 3 || s.length > 180) return '';
    if (looksLikeImageArtifact(s)) return '';
    if (/^(learn more|read more|view details|details|shop now|buy now|free trial)$/i.test(s)) return '';
    return s;
  }

  function bestAnchor(card) {
    var anchors = card.querySelectorAll('a[href]');
    var best = null;
    var bestScore = -1;
    for (var i = 0; i < anchors.length; i++) {
      var a = anchors[i];
      var title = meaningfulTitle(cleanText(a));
      var href = attr(a, 'href');
      if (!href || !title) continue;
      var score = title.length;
      if (a.querySelector('h1,h2,h3,h4,[itemprop="name"]')) score += 80;
      if ((a.getAttribute('class') || '').match(/title|name|card|product|course|recipe/i)) score += 30;
      if (score > bestScore) {
        best = { el: a, title: title, url: resolveHref(href), score: score };
        bestScore = score;
      }
    }
    return best;
  }

  function cardTitle(card, anchor) {
    if (anchor && anchor.title) return anchor.title;
    var el = firstMatch(card, [
      '[itemprop="name"]', 'h1', 'h2', 'h3', 'h4',
      '.title', '.card-title', '.product-title', '.course-title', '.recipe-title', '.name'
    ]);
    return meaningfulTitle(cleanText(el));
  }

  function cardSnippet(card, title) {
    var el = firstMatch(card, [
      '[itemprop="description"]', '.description', '.summary', '.excerpt', '.snippet',
      '.dek', '.subtitle', 'p'
    ]);
    var text = cleanText(el);
    if (!text) return null;
    if (title && text === title) return null;
    if (title && text.indexOf(title + ' ') === 0) text = collapseWhitespace(text.slice(title.length));
    if (text.length > 300) text = text.slice(0, 297).replace(/\s+\S*$/, '') + '...';
    return text || null;
  }

  function cardMeta(card, title, snippet) {
    var selectors = [
      '[itemprop="price"]', '[itemprop="duration"]', '[itemprop="cookTime"]',
      '[itemprop="prepTime"]', '[itemprop="recipeYield"]', '[itemprop="ratingValue"]',
      '.price', '.duration', '.time', '.rating', '.reviews', '.level', '.category',
      '.badge', '.tag', '.meta', 'small'
    ];
    var values = [];
    for (var s = 0; s < selectors.length; s++) {
      var nodes = [];
      try { nodes = card.querySelectorAll(selectors[s]); } catch (e) { nodes = []; }
      for (var i = 0; i < nodes.length && values.length < 8; i++) {
        var text = cleanText(nodes[i]);
        if (!text || text === title || text === snippet) continue;
        if (values.indexOf(text) === -1) values.push(text);
      }
    }
    return values;
  }

  function fieldNodes(root, selector) {
    var nodes = [];
    try {
      if (root.matches && root.matches(selector)) nodes.push(root);
    } catch (e) {}
    try {
      var descendants = root.querySelectorAll(selector);
      for (var i = 0; i < descendants.length; i++) nodes.push(descendants[i]);
    } catch (e) {}
    return nodes;
  }

  function fieldValue(root, selectors, attrs) {
    attrs = attrs || ['content', 'value'];
    for (var s = 0; s < selectors.length; s++) {
      var nodes = fieldNodes(root, selectors[s]);
      for (var i = 0; i < nodes.length; i++) {
        for (var a = 0; a < attrs.length; a++) {
          var v = attr(nodes[i], attrs[a]);
          if (v) return collapseWhitespace(v);
        }
        var text = cleanText(nodes[i]);
        if (text) return text;
      }
    }
    return null;
  }

  function normalizePrice(text, allowBare) {
    text = collapseWhitespace(text || '');
    if (!text || /free\s+trial/i.test(text)) return null;
    var currency = '(?:[$€£¥₹]|USD|CAD|AUD|GBP|EUR|JPY|INR)';
    var amount = '\\d[\\d,]*(?:\\.\\d{2})?';
    var re = new RegExp('(?:' + currency + '\\s*)?' + amount + '(?:\\s*[-–]\\s*(?:' + currency + '\\s*)?' + amount + ')?', 'i');
    var m = text.match(re);
    if (!m) return null;
    var value = collapseWhitespace(m[0]);
    if (!allowBare && !new RegExp(currency, 'i').test(value) && !/[€£¥₹$]/.test(value)) return null;
    return value || null;
  }

  function normalizeAvailability(text, loose) {
    var s = collapseWhitespace(text || '').toLowerCase();
    if (!s) return null;
    if (/outofstock|out of stock|sold out|unavailable|not available/.test(s)) return 'out_of_stock';
    if (/preorder|pre-order|pre order/.test(s)) return 'preorder';
    if (/backorder|back-order|back order/.test(s)) return 'backorder';
    if (/instock|in stock|schema\.org\/instock/.test(s)) return 'in_stock';
    if (loose && /available/.test(s)) return 'in_stock';
    return null;
  }

  function normalizeCondition(text, loose) {
    var s = collapseWhitespace(text || '').toLowerCase();
    if (!s) return null;
    if (/refurbished|renewed|reconditioned|refurbishedcondition/.test(s)) return 'refurbished';
    if (/open box|open-box|openbox/.test(s)) return 'open_box';
    if (/pre[ -]?owned|used|second hand|second-hand|usedcondition/.test(s)) return 'used';
    if (/new condition|newcondition|brand new/.test(s)) return 'new';
    if (loose && /^new$/.test(s)) return 'new';
    if (/damaged|for parts|damagedcondition/.test(s)) return 'damaged';
    return null;
  }

  function metaHasSameMeaning(value, normalized) {
    if (!value || !normalized) return false;
    var v = collapseWhitespace(value).toLowerCase();
    var n = collapseWhitespace(normalized).toLowerCase();
    return v === n || v.indexOf(n) !== -1 || n.indexOf(v) !== -1;
  }

  function commerceFields(card, title, snippet) {
    var meta = cardMeta(card, title, snippet);
    var priceRaw = fieldValue(card, [
      '[itemprop="price"]', '[itemprop="lowPrice"]', '[itemprop="highPrice"]',
      '[data-price]', '[class*="price"]', '[class*="Price"]', '.sale-price', '.amount'
    ], ['content', 'value', 'data-price']);
    var availabilityRaw = fieldValue(card, [
      '[itemprop="availability"]', '[class*="availability"]', '[class*="Availability"]',
      '[class*="stock"]', '[class*="Stock"]', '[data-availability]'
    ], ['content', 'value', 'data-availability', 'href', 'aria-label', 'title']);
    var conditionRaw = fieldValue(card, [
      '[itemprop="itemCondition"]', '[class*="condition"]', '[class*="Condition"]',
      '[class*="quality"]', '[class*="Quality"]', '[data-condition]'
    ], ['content', 'value', 'data-condition', 'href', 'aria-label', 'title']);

    var price = normalizePrice(priceRaw, true);
    var availability = normalizeAvailability(availabilityRaw, true);
    var condition = normalizeCondition(conditionRaw, true);
    for (var i = 0; i < meta.length; i++) {
      if (!price) price = normalizePrice(meta[i], false);
      if (!availability) availability = normalizeAvailability(meta[i], true);
      if (!condition) condition = normalizeCondition(meta[i], true);
    }
    var cardText = cleanText(card);
    if (!availability) availability = normalizeAvailability(cardText, false);
    if (!condition) condition = normalizeCondition(cardText, false);

    var filtered = [];
    for (var m = 0; m < meta.length; m++) {
      var v = meta[m];
      if (v === title || v === snippet) continue;
      if (metaHasSameMeaning(v, price) || normalizePrice(v, false) === price) continue;
      if (normalizeAvailability(v, true) === availability) continue;
      if (normalizeCondition(v, true) === condition) continue;
      if (filtered.indexOf(v) === -1) filtered.push(v);
    }
    return {
      price: price || null,
      availability: availability || null,
      condition: condition || null,
      meta: filtered
    };
  }

  globalThis.__cardCommerceFields = commerceFields;

  function cardScore(card, title, anchor, kind) {
    var score = 0;
    var tag = (card.tagName || '').toLowerCase();
    var cls = (card.getAttribute('class') || '') + ' ' + (card.getAttribute('itemtype') || '');
    if (title) score += 40;
    if (anchor && anchor.url) score += 25;
    if (tag === 'article') score += 15;
    if (/card|product|course|recipe|listing|result|item/i.test(cls)) score += 20;
    if (kind && cls.toLowerCase().indexOf(kind.toLowerCase()) !== -1) score += 15;
    if (card.querySelector('img[alt]')) score += 5;
    var textLen = cleanText(card).length;
    if (textLen >= 30 && textLen <= 800) score += 10;
    if (textLen > 1600) score -= 25;
    return score;
  }
  // extract_table — pull a <table> into {headers, rows}. Headers come
  // from <thead><th>...</th></thead> if present, else the first <tr>'s
  // <th> cells. Each subsequent <tr>'s <td> cells become a row dict
  // keyed by header (or 'col_N' if no header for that column).
  globalThis.__extractTable = function (selector) {
    var table = document.querySelector(selector);
    if (!table) return null;
    var headers = [];
    var thead = table.querySelector('thead');
    var headerRow = thead ? thead.querySelector('tr') : null;
    if (!headerRow) {
      // Look for the first <tr> that has <th> cells.
      var trs = table.querySelectorAll('tr');
      for (var i = 0; i < trs.length; i++) {
        if (trs[i].querySelector('th')) { headerRow = trs[i]; break; }
      }
    }
    if (headerRow) {
      var hcells = headerRow.querySelectorAll('th');
      for (var hi = 0; hi < hcells.length; hi++) {
        headers.push((hcells[hi].textContent || '').trim());
      }
    }
    var rows = [];
    var bodyTrs = table.querySelectorAll('tbody tr');
    if (!bodyTrs.length) {
      bodyTrs = [];
      var allTrs = table.querySelectorAll('tr');
      for (var ti = 0; ti < allTrs.length; ti++) {
        if (allTrs[ti] !== headerRow) bodyTrs.push(allTrs[ti]);
      }
    }
    for (var r = 0; r < bodyTrs.length; r++) {
      var tds = bodyTrs[r].querySelectorAll('td');
      if (!tds.length) continue;
      var rowObj = {};
      for (var c = 0; c < tds.length; c++) {
        var key = headers[c] || ('col_' + c);
        rowObj[key] = (tds[c].textContent || '').trim();
      }
      rows.push(rowObj);
    }
    return { headers: headers, rows: rows, row_count: rows.length };
  };

  // extract_list — pull a repeated card pattern into [{...}, {...}].
  // `itemSelector` matches each card; `fields` maps field names to
  // sub-selectors. Field spec shapes:
  //   "css selector"          -> textContent of first match
  //   "css selector @attr"    -> value of `attr` on first match
  //   ["css selector", "@attr"] -> same, tuple form
  // If the sub-selector returns null, the field value is null.
  globalThis.__extractList = function (itemSelector, fields, limit) {
    limit = limit || 1000;
    var items = document.querySelectorAll(itemSelector);
    var out = [];
    var fieldNames = Object.keys(fields || {});
    for (var i = 0; i < items.length && i < limit; i++) {
      var item = items[i];
      var rec = {};
      for (var fi = 0; fi < fieldNames.length; fi++) {
        var name = fieldNames[fi];
        var spec = fields[name];
        var sel = null;
        var attr = null;
        if (typeof spec === 'string') {
          var m = spec.match(/^(.+?)\s*@(\S+)$/);
          if (m) { sel = m[1].trim(); attr = m[2]; }
          else { sel = spec; }
        } else if (Array.isArray(spec) && spec.length === 2) {
          sel = spec[0];
          attr = String(spec[1]).replace(/^@/, '');
        } else {
          rec[name] = null;
          continue;
        }
        var el = sel ? item.querySelector(sel) : item;
        if (!el) { rec[name] = null; continue; }
        if (attr) {
          rec[name] = el.getAttribute(attr);
        } else {
          rec[name] = cleanText(el);
        }
      }
      out.push(rec);
    }
    return out;
  };

  globalThis.__extractCards = function (selector, limit, kind) {
    limit = limit || 50;
    var candidates = [];
    var selectors = selector ? [selector] : [
      'article', '[itemscope]', '[itemtype*="Product"]', '[itemtype*="Recipe"]',
      '.card', '.product', '.course', '.recipe', '.listing', '.result',
      '[class*="card"]', '[class*="product"]', '[class*="course"]',
      '[class*="recipe"]', '[class*="listing"]', 'li'
    ];
    for (var s = 0; s < selectors.length; s++) {
      var nodes = [];
      try { nodes = document.querySelectorAll(selectors[s]); } catch (e) { nodes = []; }
      for (var i = 0; i < nodes.length; i++) {
        if (candidates.indexOf(nodes[i]) === -1) candidates.push(nodes[i]);
      }
    }

    var rows = [];
    var seen = {};
    for (var c = 0; c < candidates.length; c++) {
      var card = candidates[c];
      var anchor = bestAnchor(card);
      var title = cardTitle(card, anchor);
      if (!title) continue;
      var url = anchor && anchor.url || resolveHref(attr(firstMatch(card, ['a[href]']), 'href'));
      var snippet = cardSnippet(card, title);
      var img = firstMatch(card, ['img']);
      var imageAlt = collapseWhitespace(attr(img, 'alt') || '');
      if (looksLikeImageArtifact(imageAlt)) imageAlt = '';
      var score = cardScore(card, title, anchor, kind);
      var commerce = commerceFields(card, title, snippet);
      if (!selector && score < 55) continue;
      var key = (url || '') + '\n' + title;
      if (seen[key]) continue;
      seen[key] = true;
      rows.push({
        title: title,
        url: url || null,
        snippet: snippet,
        price: commerce.price,
        condition: commerce.condition,
        availability: commerce.availability,
        meta: commerce.meta,
        image_alt: imageAlt || null,
        score: score
      });
    }
    rows.sort(function (a, b) { return b.score - a.score; });
    if (rows.length > limit) rows.length = limit;
    return rows;
  };

  globalThis.__extract = function (opts) {
    opts = opts || {};
    var requested = opts.strategy; // optional: force a specific strategy
    var all = [
      ['json_ld', strategyJsonLd],
      ['next_data', strategyNextData],
      ['rsc_payload', strategyRscPayload],         // Next.js App Router
      ['nuxt_data', strategyNuxtData],
      ['json_in_script', strategyJsonInScript],   // Magento, Shopify, etc.
      ['og_meta', strategyOgMeta],
      ['microdata', strategyMicrodata],
      ['text_main', strategyTextMain],
    ];
    if (requested) {
      for (var i = 0; i < all.length; i++) {
        if (all[i][0] === requested) {
          var r = all[i][1]();
          return r || { strategy: requested, confidence: 0, data: null,
                        hint: 'requested strategy returned no data' };
        }
      }
      return { strategy: requested, confidence: 0, data: null,
               hint: 'unknown strategy ' + requested };
    }
    var tried = [];
    var best = null;
    var hits = [];  // strategies with confidence >= 0.5, full data carried
    for (var j = 0; j < all.length; j++) {
      var name = all[j][0], fn = all[j][1];
      try {
        var res = fn();
        tried.push({ strategy: name, confidence: res ? res.confidence : 0,
                     hit: !!res });
        if (res && (!best || res.confidence > best.confidence)) best = res;
        if (res && res.confidence >= 0.5) {
          hits.push({ strategy: name, confidence: res.confidence, data: res.data });
        }
      } catch (e) {
        tried.push({ strategy: name, confidence: 0, hit: false,
                     error: String(e && e.message || e) });
      }
    }
    if (!best) return { strategy: 'none', confidence: 0, data: null, tried: tried };
    // Sort hits by confidence desc; cap at 5 to bound payload size on pages
    // where many strategies hit (Polymarket: json_ld + next_data +
    // json_in_script + og_meta all return data).
    hits.sort(function(a, b) { return b.confidence - a.confidence; });
    if (hits.length > 5) hits.length = 5;
    best.tried = tried;
    best.all_hits = hits;
    return best;
  };
})();

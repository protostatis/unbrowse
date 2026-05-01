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
    return {
      strategy: 'next_data',
      confidence: page ? 0.9 : 0.7,
      data: page || parsed,
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

  globalThis.__extract = function (opts) {
    opts = opts || {};
    var requested = opts.strategy; // optional: force a specific strategy
    var all = [
      ['json_ld', strategyJsonLd],
      ['next_data', strategyNextData],
      ['nuxt_data', strategyNuxtData],
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
    for (var j = 0; j < all.length; j++) {
      var name = all[j][0], fn = all[j][1];
      try {
        var res = fn();
        tried.push({ strategy: name, confidence: res ? res.confidence : 0,
                     hit: !!res });
        if (res && (!best || res.confidence > best.confidence)) best = res;
      } catch (e) {
        tried.push({ strategy: name, confidence: 0, hit: false,
                     error: String(e && e.message || e) });
      }
    }
    if (!best) return { strategy: 'none', confidence: 0, data: null, tried: tried };
    best.tried = tried;
    return best;
  };
})();

/* Zem browser-pane bridge.
 *
 * Injected into every proxied page by the browser addon's proxy. It:
 *  - intercepts link clicks + GET form submits so navigation stays inside the
 *    iframe (through the proxy) instead of loading the target site directly
 *  - reports title / resolved URL / scroll position to the parent pane
 *  - captures console.log/warn/error, uncaught errors, and fetch/XHR traffic
 *    so the pane can show a DevTools-style drawer
 *  - applies an initial scroll position passed in by the pane (scroll restore)
 *
 * Communication is one-way push via `window.parent.postMessage`; the parent
 * pane drives navigation by rewriting the iframe src.
 */
(function () {
  if (window.__zemBridge) return;
  window.__zemBridge = true;

  var targetUrl = window.__zemTarget || document.baseURI || location.href;
  // The pane injects a <base href> pointing at the *target* origin, so a
  // relative prefix would resolve against the wrong host. Derive the proxy
  // origin from the actual iframe location instead.
  var proxyPrefix = window.__zemProxyPrefix || location.origin + '/proxy?url=';

  function post(type, data) {
    try {
      window.parent.postMessage(
        { __zem: true, type: type, data: data || {}, url: targetUrl },
        '*',
      );
    } catch (e) {
      /* parent gone — ignore */
    }
  }

  function encode(u) {
    return proxyPrefix + encodeURIComponent(u);
  }

  function resolveHref(href) {
    var a = document.createElement('a');
    a.href = href;
    return a.href; // absolute
  }

  function isExternal(url) {
    return /^(mailto|tel|javascript|data|blob|about|file|ws|wss):/i.test(url);
  }

  // Route an http(s) request through the proxy so the iframe's origin (the
  // proxy) serves every sub-resource — otherwise fonts/APIs load cross-origin
  // and hit CORS errors. Returns the rewritten absolute URL.
  function routeThroughProxy(abs) {
    if (!abs || isExternal(abs)) return abs;
    // Only URLs already on the /proxy route are truly proxied — everything else
    // on the proxy origin (e.g. Next.js building location.origin + '/_next/…')
    // would 404 because the proxy only serves /proxy.
    if (abs.indexOf(location.origin + '/proxy?url=') === 0) return abs;
    // Absolute URLs the page built from its own origin must be translated to
    // the real target origin before proxying. The injected <base href> points
    // at the target, so document.baseURI gives us its origin.
    if (abs.indexOf(location.origin) === 0) {
      try {
        abs = new URL(document.baseURI).origin + abs.slice(location.origin.length);
      } catch (e) {
        /* leave abs as-is */
      }
    }
    return encode(abs);
  }

  // ── Navigation interception ─────────────────────────────────────────────
  document.addEventListener(
    'click',
    function (e) {
      var a = e.target.closest ? e.target.closest('a') : null;
      if (!a) return;
      var href = a.getAttribute('href');
      if (!href || isExternal(href)) return;

      // Same-document fragment links should just scroll, not reload the page.
      if (href.charAt(0) === '#') return;

      var target = a.target;
      // Middle-click / cmd+click / target=_blank → open in external browser.
      if (e.button === 1 || e.metaKey || e.ctrlKey || target === '_blank') {
        e.preventDefault();
        post('open-external', { url: resolveHref(href) });
        return;
      }

      e.preventDefault();
      var abs = resolveHref(href);
      post('navigate', { url: abs });
      location.href = encode(abs);
    },
    true,
  );

  document.addEventListener(
    'submit',
    function (e) {
      var form = e.target;
      if (!form || form.method && form.method.toLowerCase() !== 'get') {
        // POST forms can't ride the GET proxy — open in external browser.
        e.preventDefault();
        post('open-external', { url: form.action || location.href });
        return;
      }
      // `location.href` inside the iframe is the proxy URL; the injected
      // <base href> points at the real target, so use document.baseURI.
      var action = form.action || document.baseURI || location.href;
      var data = new FormData(form);
      var qs = new URLSearchParams(data).toString();
      var sep = action.indexOf('?') >= 0 ? '&' : '?';
      var abs = action + sep + qs;
      e.preventDefault();
      post('navigate', { url: abs });
      location.href = encode(abs);
    },
    true,
  );

  // ── History shim (SPA support) ─────────────────────────────────────────
  // The document lives on the proxy origin, so SPA routers (Next.js, React
  // Router) hit two failure modes when they call history.pushState/replaceState:
  //   - absolute URLs they derive from location + their own origin are
  //     cross-origin here and throw SecurityError (the aparat.com crash)
  //   - relative URLs resolve to raw proxy paths (`/proxy?url=…` vs `/foo`)
  //     that break reloads
  // Map every URL to the /proxy?url= form so the document URL always
  // round-trips through the proxy.
  function proxyHistoryUrl(url) {
    if (url == null) return url;
    var abs;
    try { abs = new URL(String(url), document.baseURI).href; } catch (e) { return url; }
    var origin;
    try { origin = new URL(abs).origin; } catch (e) { return url; }
    if (origin === location.origin) {
      // Same-origin: a real /proxy?url= URL is fine. A raw proxy path the site
      // built from location.origin/href (e.g. location.origin + '/some/path')
      // is mangled — translate its origin to the real target origin so the
      // intended path is preserved rather than falling back to the current page.
      if (abs.indexOf(location.origin + '/proxy?url=') === 0) return url;
      try {
        return proxyPrefix + encodeURIComponent(
          new URL(document.baseURI).origin + abs.slice(location.origin.length),
        );
      } catch (e) { return url; }
    }
    // Cross-origin absolute — the site's "real" URL. The mangled
    // <target>/proxy?url=… form (site resolved its own pathname against its
    // origin) can't be trusted, so fall back to the current real page.
    if (/\/proxy\?url=/.test(abs)) abs = document.baseURI;
    return proxyPrefix + encodeURIComponent(abs);
  }
  function realFromMapped(mapped) {
    if (!mapped || typeof mapped !== 'string') return null;
    var marker = '/proxy?url=';
    var idx = mapped.indexOf(marker);
    if (idx < 0) return null;
    try { return decodeURIComponent(mapped.slice(idx + marker.length)); } catch (e) { return null; }
  }
  var origPushState = history.pushState;
  var origReplaceState = history.replaceState;
  function patchHistory(orig, isReplace) {
    return function (state, title, url) {
      var mapped = proxyHistoryUrl(url);
      var real = realFromMapped(mapped);
      var result = orig.call(this, state, title, mapped);
      // Keep the pane's address bar + history in sync with SPA navigation.
      if (real) post('navigate', { url: real, replace: !!isReplace });
      return result;
    };
  }
  history.pushState = patchHistory(origPushState, false);
  history.replaceState = patchHistory(origReplaceState, true);

  // ── Title + URL reporting ───────────────────────────────────────────────
  function reportTitle() {
    post('title', { title: document.title || '' });
  }
  document.addEventListener('DOMContentLoaded', reportTitle);
  window.addEventListener('load', function () {
    reportTitle();
    // Scroll restore
    var restore = parseInt(window.__zemScrollY || '0', 10);
    if (restore > 0) window.scrollTo(0, restore);
    // Report initial scroll once the page settles.
    setTimeout(reportScroll, 250);
  });

  // ── Scroll reporting (throttled) ────────────────────────────────────────
  var scrollTimer = null;
  function reportScroll() {
    post('scroll', { y: Math.max(window.pageYOffset || 0, 0) });
  }
  window.addEventListener(
    'scroll',
    function () {
      if (scrollTimer) return;
      scrollTimer = setTimeout(function () {
        scrollTimer = null;
        reportScroll();
      }, 150);
    },
    { passive: true },
  );

  // ── Console capture ─────────────────────────────────────────────────────
  ['log', 'info', 'warn', 'error', 'debug'].forEach(function (level) {
    var orig = console[level];
    console[level] = function () {
      var args = Array.prototype.map.call(arguments, function (a) {
        try {
          if (typeof a === 'string') return a;
          if (a instanceof Error) return a.message;
          var json = JSON.stringify(a);
          return json === undefined ? String(a) : json;
        } catch (e) {
          return String(a);
        }
      });
      post('console', { level: level, args: args });
      orig.apply(console, arguments);
    };
  });

  window.addEventListener('error', function (e) {
    post('error', {
      message: e.message || String(e),
      file: e.filename,
      line: e.lineno,
    });
  });
  window.addEventListener('unhandledrejection', function (e) {
    var reason = e.reason;
    post('error', {
      message:
        'Unhandled rejection: ' +
        (reason && reason.message ? reason.message : String(reason)),
    });
  });

  // ── Network capture + proxying (fetch + XHR) ─────────────────────────
  var origFetch = window.fetch;
  window.fetch = function (input, init) {
    var start = Date.now();
    var realUrl = typeof input === 'string' ? input : input && input.url ? input.url : String(input);
    var abs = resolveHref(realUrl);
    var target = input;
    var targetInit = init;

    // Rewrite the URL through the proxy so the request is same-origin.
    var proxied = routeThroughProxy(abs);
    if (proxied !== abs) {
      if (typeof input === 'string') {
        target = proxied;
      } else if (typeof Request === 'function' && input instanceof Request) {
        try {
          target = new Request(proxied, {
            method: input.method,
            headers: input.headers,
            body: input.body,
            mode: input.mode,
            credentials: input.credentials,
            cache: input.cache,
            redirect: input.redirect,
          });
        } catch (e) {
          target = input;
        }
      }
    }

    return origFetch
      .call(this, target, targetInit)
      .then(function (res) {
        post('network', {
          method: (init && init.method) || (input && input.method) || 'GET',
          url: realUrl,
          status: res.status,
          ms: Date.now() - start,
        });
        return res;
      })
      .catch(function (err) {
        post('network', {
          method: (init && init.method) || (input && input.method) || 'GET',
          url: realUrl,
          status: 0,
          ms: Date.now() - start,
          error: String(err),
        });
        throw err;
      });
  };

  var origOpen = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (method, url, async, user, pass) {
    this.__zemMethod = method;
    this.__zemUrl = url;
    this.__zemStart = Date.now();
    if (typeof url === 'string') {
      var abs = resolveHref(url);
      var proxied = routeThroughProxy(abs);
      if (proxied !== abs) url = proxied;
    }
    return origOpen.call(this, method, url, async, user, pass);
  };
  var origSend = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.send = function () {
    this.addEventListener('loadend', function () {
      post('network', {
        method: this.__zemMethod || 'GET',
        url: this.__zemUrl || '',
        status: this.status,
        ms: Date.now() - (this.__zemStart || Date.now()),
      });
    });
    return origSend.apply(this, arguments);
  };

  // ── Focus signaling ─────────────────────────────────────────────────────
  // Clicks inside the iframe never bubble to the parent document, so the
  // parent pane's click-to-focus can't fire (and `focusin` is unreliable in
  // Tauri's WebKit). Signal the host directly on any user interaction so it
  // can focus this pane. Throttled — we only need one signal per interaction.
  var lastFocusAt = 0;
  function signalFocus() {
    var now = Date.now();
    if (now - lastFocusAt < 150) return;
    lastFocusAt = now;
    post('focus', {});
  }
  ['pointerdown', 'mousedown', 'keydown', 'touchstart'].forEach(function (evt) {
    window.addEventListener(evt, signalFocus, { capture: true });
  });

  // ── Let the pane know we're alive + give it the real URL ───────────────
  // The bridge runs in the <head>, so wait for the document to be interactive
  // before announcing readiness (otherwise the pane clears its loading bar
  // before the page has painted).
  function reportReady() {
    post('ready', {
      url: targetUrl,
      title: document.title || '',
      href: location.href,
    });
  }
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', reportReady);
  } else {
    reportReady();
  }
})();

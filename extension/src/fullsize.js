// From a thumbnail's address to the original's, on the sites and image CDNs
// that put the size in the address. `FisgaFullsize.candidates(url)` gives the
// addresses to try, best first, without the one given; the gallery checks each
// (it must answer with an image) and keeps the first that does, else the
// thumbnail. Signed addresses (Instagram, Facebook, DeviantArt) are left
// alone: changing them breaks the signature, and their srcset already has the
// largest size.
(() => {
  "use strict";

  function parse(url) {
    try {
      return new URL(url);
    } catch {
      return null;
    }
  }
  const hostIs = (u, ...hosts) => hosts.some((h) => u.hostname === h || u.hostname.endsWith("." + h));
  /** The same address without these query parameters. */
  function without(u, names) {
    const copy = new URL(u.href);
    for (const name of names) copy.searchParams.delete(name);
    return copy.href.replace(/\?$/, "");
  }
  /** The same address with its path changed by `fn`. */
  function withPath(u, fn) {
    const copy = new URL(u.href);
    copy.pathname = fn(copy.pathname);
    return copy.href;
  }

  const RULES = [
    {
      // X/Twitter: ?format=jpg&name=small → name=orig; profile pictures _normal → original.
      name: "twitter",
      apply(u) {
        if (!hostIs(u, "pbs.twimg.com")) return [];
        if (/^\/profile_(images|banners)\//.test(u.pathname)) {
          return [withPath(u, (p) => p.replace(/_(normal|bigger|mini|reasonably_small|\d+x\d+)(\.\w+)$/, "$2"))];
        }
        if (!/^\/(media|ext_tw_video_thumb|amplify_video_thumb|tweet_video_thumb)\//.test(u.pathname)) return [];
        const copy = new URL(u.href);
        const legacy = /^(.*?)(\.(jpe?g|png|webp))(?::\w+)?$/i.exec(copy.pathname);
        if (legacy) {
          copy.pathname = legacy[1];
          copy.searchParams.set("format", legacy[3].toLowerCase() === "jpeg" ? "jpg" : legacy[3].toLowerCase());
        }
        copy.searchParams.set("name", "orig");
        return [copy.href];
      },
    },
    {
      // Wikimedia: /thumb/a/ab/File.jpg/220px-File.jpg → /a/ab/File.jpg.
      name: "wikimedia",
      apply(u) {
        if (!hostIs(u, "upload.wikimedia.org")) return [];
        const m = /^(.*)\/thumb\/([0-9a-f]\/[0-9a-f]{2}\/[^/]+)\/[^/]+$/.exec(u.pathname);
        return m ? [withPath(u, () => `${m[1]}/${m[2]}`)] : [];
      },
    },
    {
      // MediaWiki file pages (/wiki/File:x.jpg, Ficheiro:, Arquivo:, Datei:...) →
      // Special:FilePath, which redirects to the file itself (on any wiki).
      name: "mediawiki-file",
      apply(u) {
        if (!hostIs(u, "wikipedia.org", "wikimedia.org", "wiktionary.org", "wikiquote.org", "wikibooks.org", "wikisource.org", "wikivoyage.org", "wikinews.org", "mediawiki.org", "wikidata.org")) {
          return [];
        }
        const m = /^\/wiki\/[^/:]+:([^/]+\.\w{2,5})$/.exec(u.pathname);
        return m ? [`${u.origin}/wiki/Special:FilePath/${m[1]}`] : [];
      },
    },
    {
      // Pinterest: /236x/.. /474x/.. /736x/.. → /originals/.. (then the largest fixed size).
      name: "pinterest",
      apply(u) {
        if (!hostIs(u, "pinimg.com")) return [];
        const m = /^\/(\d+x\d*|[a-z]*\d+x\d*_RS|\d+x)\/(.+)$/.exec(u.pathname);
        if (!m) return [];
        return [withPath(u, () => `/originals/${m[2]}`), withPath(u, () => `/736x/${m[2]}`)];
      },
    },
    {
      // Imgur: abcdefgh.jpg with a size letter (s, b, t, m, l, h) after the id → without it.
      name: "imgur",
      apply(u) {
        if (!hostIs(u, "i.imgur.com")) return [];
        const m = /^\/([A-Za-z0-9]{7}|[A-Za-z0-9]{5})[sbtmlh](\.\w+)$/.exec(u.pathname);
        return m ? [withPath(u, () => `/${m[1]}${m[2]}`)] : [];
      },
    },
    {
      // Reddit: preview.redd.it/name.jpg?width=640&... → i.redd.it/name.jpg.
      name: "reddit",
      apply(u) {
        if (u.hostname !== "preview.redd.it") return [];
        const name = u.pathname.split("/").pop();
        if (!/\.(jpe?g|png|gif|webp)$/i.test(name)) return [];
        return [`https://i.redd.it/${name}`];
      },
    },
    {
      // Flickr: id_secret_m.jpg → id_secret_b.jpg (1024 px; larger ones use another secret).
      name: "flickr",
      apply(u) {
        if (!hostIs(u, "staticflickr.com")) return [];
        const m = /^(.*\/\d+_[0-9a-f]+)(?:_[sqtmnwzc])?(\.jpg)$/.exec(u.pathname);
        return m ? [withPath(u, () => `${m[1]}_b${m[2]}`)] : [];
      },
    },
    {
      // Tumblr: /s640x960/ → /s2048x3072/; old _500.jpg → _1280.jpg.
      name: "tumblr",
      apply(u) {
        if (!hostIs(u, "media.tumblr.com")) return [];
        if (/\/s\d+x\d+(\/|$)/.test(u.pathname)) return [withPath(u, (p) => p.replace(/\/s\d+x\d+(\/|$)/, "/s2048x3072$1"))];
        if (/_\d{2,4}(\.\w+)$/.test(u.pathname)) return [withPath(u, (p) => p.replace(/_\d{2,4}(\.\w+)$/, "_1280$1"))];
        return [];
      },
    },
    {
      // Google (Photos, Blogger, Play, YouTube avatars): =w400-h300-... → =s0; /s400/ → /s0/.
      name: "google",
      apply(u) {
        if (!hostIs(u, "googleusercontent.com", "ggpht.com", "bp.blogspot.com", "blogger.googleusercontent.com")) return [];
        if (/=[swh]\d+[^/]*$/.test(u.pathname)) return [withPath(u, (p) => p.replace(/=[swh]\d+[^/]*$/, "=s0"))];
        if (/\/[swh]\d+(-[a-z0-9-]+)?\//.test(u.pathname)) return [withPath(u, (p) => p.replace(/\/[swh]\d+(-[a-z0-9-]+)?\//, "/s0/"))];
        return [];
      },
    },
    {
      // YouTube thumbnails: hqdefault/mqdefault/sddefault → maxresdefault.
      name: "youtube",
      apply(u) {
        if (!hostIs(u, "ytimg.com")) return [];
        const m = /^\/vi(?:_webp)?\/([\w-]{11})\/(?:hq720|(?:hq|mq|sd|oar)?default|(?:hq|mq|sd|oar)\d)\.(?:jpg|webp)$/.exec(u.pathname);
        return m ? [`https://i.ytimg.com/vi/${m[1]}/maxresdefault.jpg`, `https://i.ytimg.com/vi/${m[1]}/hqdefault.jpg`] : [];
      },
    },
    {
      // Amazon: ID._AC_SX300_.jpg → ID.jpg.
      name: "amazon",
      apply(u) {
        if (!hostIs(u, "media-amazon.com", "ssl-images-amazon.com", "images-amazon.com")) return [];
        return /\._[^/]*_\.(\w+)$/.test(u.pathname) ? [withPath(u, (p) => p.replace(/\._[^/]*_\.(\w+)$/, ".$1"))] : [];
      },
    },
    {
      // eBay: s-l300.jpg → s-l1600.jpg.
      name: "ebay",
      apply(u) {
        if (!hostIs(u, "ebayimg.com")) return [];
        return /\/s-l\d+\.(\w+)$/.test(u.pathname) ? [withPath(u, (p) => p.replace(/\/s-l\d+\.(\w+)$/, "/s-l1600.$1"))] : [];
      },
    },
    {
      // Etsy: il_340x270.ID.jpg → il_fullxfull.ID.jpg.
      name: "etsy",
      apply(u) {
        if (!hostIs(u, "etsystatic.com")) return [];
        return /\/il_\d+x[\dN]+\./.test(u.pathname) ? [withPath(u, (p) => p.replace(/\/il_\d+x[\dN]+\./, "/il_fullxfull."))] : [];
      },
    },
    {
      // ArtStation: /smaller_square/ /small/ /medium/ /large/ → /4k/, then /large/.
      name: "artstation",
      apply(u) {
        if (!hostIs(u, "artstation.com") || !/\/images\//.test(u.pathname)) return [];
        const size = /\/(smaller_square|small_square|micro_square|small|medium|large|4k)\//;
        if (!size.test(u.pathname)) return [];
        return ["4k", "large"].map((s) => withPath(u, (p) => p.replace(size, `/${s}/`)));
      },
    },
    {
      // Behance: /projects/404/ or /project_modules/max_1200/ → /source/, then max_3840.
      name: "behance",
      apply(u) {
        if (!hostIs(u, "behance.net")) return [];
        const m = /^\/(projects|project_modules)\/[^/]+\/(.+)$/.exec(u.pathname);
        if (!m) return [];
        return [withPath(u, () => `/${m[1]}/source/${m[2]}`), withPath(u, () => `/${m[1]}/max_3840/${m[2]}`)];
      },
    },
    {
      // WordPress: name-300x200.jpg → name.jpg; Jetpack (i0.wp.com) ?resize=/?w= → without.
      name: "wordpress",
      apply(u) {
        const out = [];
        if (/\/wp-content\/uploads\//.test(u.pathname) && /-\d+x\d+(\.\w+)$/.test(u.pathname)) {
          out.push(withPath(u, (p) => p.replace(/-\d+x\d+(\.\w+)$/, "$1")));
        }
        if (hostIs(u, "wp.com") || /\/wp-content\//.test(u.pathname)) {
          const plain = without(u, ["resize", "w", "h", "fit", "crop", "quality", "ssl", "strip", "zoom"]);
          if (plain !== u.href) out.push(plain);
        }
        return out;
      },
    },
    {
      // Shopify: file_200x200.jpg / file_x400.jpg / @2x / ?width= → the uploaded file.
      name: "shopify",
      apply(u) {
        if (!hostIs(u, "cdn.shopify.com", "shopify.com") && !/\/cdn\/shop\//.test(u.pathname)) return [];
        const copy = new URL(without(u, ["width", "height", "crop"]));
        copy.pathname = copy.pathname.replace(/_(\d+x\d*|x\d+|pico|icon|thumb|small|compact|medium|large|grande)(_crop_\w+)?(@\dx)?(\.\w+)$/, "$4");
        return copy.href !== u.href ? [copy.href] : [];
      },
    },
    {
      // Squarespace: ?format=500w → ?format=original.
      name: "squarespace",
      apply(u) {
        if (!hostIs(u, "squarespace-cdn.com", "squarespace.com")) return [];
        const copy = new URL(u.href);
        copy.searchParams.set("format", "original");
        return copy.href !== u.href ? [copy.href] : [];
      },
    },
    {
      // Wix: /media/abc~mv2.jpg/v1/fill/w_300,.../abc~mv2.jpg → /media/abc~mv2.jpg.
      name: "wix",
      apply(u) {
        if (!hostIs(u, "wixstatic.com")) return [];
        const m = /^(\/media\/[^/]+)\/v1\/.+$/.exec(u.pathname);
        return m ? [withPath(u, () => m[1])] : [];
      },
    },
    {
      // Cloudinary: /image/upload/w_300,c_fill/v123/x.jpg → /image/upload/v123/x.jpg.
      name: "cloudinary",
      apply(u) {
        if (!hostIs(u, "cloudinary.com")) return [];
        const m = /^(.*\/(?:image|video)\/upload\/)((?:[a-z]{1,3}_[^/]+\/)+)(v\d+\/.+|[^/]+\.\w+)$/.exec(u.pathname);
        return m ? [withPath(u, () => `${m[1]}${m[3]}`)] : [];
      },
    },
    {
      // imgix and other CDNs that resize by parameters (Unsplash, Contentful, Sanity, Storyblok, DatoCMS).
      name: "imgix",
      apply(u) {
        if (!hostIs(u, "imgix.net", "images.unsplash.com", "plus.unsplash.com", "images.ctfassets.net", "cdn.sanity.io", "datocms-assets.com", "images.prismic.io")) {
          return [];
        }
        const plain = without(u, ["w", "h", "width", "height", "fit", "crop", "dpr", "q", "auto", "fm", "cs", "rect", "max-w", "max-h"]);
        return plain !== u.href ? [plain] : [];
      },
    },
    {
      // Medium: /v2/resize:fit:700/format:webp/1*x.png → /v2/1*x.png.
      name: "medium",
      apply(u) {
        if (!hostIs(u, "miro.medium.com", "cdn-images-1.medium.com")) return [];
        if (/^\/v2\/(?:[a-z]+:[^/]+\/)+/.test(u.pathname)) return [withPath(u, (p) => p.replace(/^\/v2\/(?:[a-z]+:[^/]+\/)+/, "/v2/"))];
        if (/^\/max\/\d+\//.test(u.pathname)) return [withPath(u, (p) => p.replace(/^\/max\/\d+\//, "/"))];
        return [];
      },
    },
    {
      // AliExpress: x.jpg_220x220.jpg / x.jpg_Q90.jpg_.webp → x.jpg.
      name: "aliexpress",
      apply(u) {
        if (!hostIs(u, "alicdn.com")) return [];
        const m = /^(.*?\.(?:jpe?g|png|webp))_.+$/i.exec(u.pathname);
        return m ? [withPath(u, () => m[1])] : [];
      },
    },
  ];

  function candidates(url) {
    const u = parse(url);
    if (!u || !/^https?:$/.test(u.protocol)) return [];
    const out = [];
    for (const rule of RULES) {
      let found = [];
      try {
        found = rule.apply(u);
      } catch {}
      for (const candidate of found) if (candidate && candidate !== url && !out.includes(candidate)) out.push(candidate);
    }
    return out;
  }

  const api = { candidates, rules: RULES.map((r) => r.name) };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else globalThis.FisgaFullsize = api;
})();

// The thumbnail → original rules of the media gallery (src/fullsize.js).
//   node --test "extension/test/*.test.mjs"   (Node 22+; Node 20: node --test extension/test/)
import test from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { candidates } = require("../src/fullsize.js");

const first = (url) => candidates(url)[0];

test("X/Twitter: the original size", () => {
  assert.equal(first("https://pbs.twimg.com/media/GaBcDeF?format=jpg&name=small"), "https://pbs.twimg.com/media/GaBcDeF?format=jpg&name=orig");
  assert.equal(first("https://pbs.twimg.com/media/GaBcDeF.png:large"), "https://pbs.twimg.com/media/GaBcDeF?format=png&name=orig");
  assert.equal(first("https://pbs.twimg.com/profile_images/1/abc_normal.jpg"), "https://pbs.twimg.com/profile_images/1/abc.jpg");
});

test("Wikimedia: the file, not its thumbnail", () => {
  assert.equal(
    first("https://upload.wikimedia.org/wikipedia/commons/thumb/a/ab/Gato_preto.jpg/220px-Gato_preto.jpg"),
    "https://upload.wikimedia.org/wikipedia/commons/a/ab/Gato_preto.jpg",
  );
  assert.deepEqual(candidates("https://upload.wikimedia.org/wikipedia/commons/a/ab/Gato_preto.jpg"), []);
  assert.equal(first("https://pt.wikipedia.org/wiki/Ficheiro:Siamese_cat.jpg"), "https://pt.wikipedia.org/wiki/Special:FilePath/Siamese_cat.jpg");
  assert.equal(first("https://en.wikipedia.org/wiki/File:Felis_catus.ogv"), "https://en.wikipedia.org/wiki/Special:FilePath/Felis_catus.ogv");
  assert.deepEqual(candidates("https://pt.wikipedia.org/wiki/Gato"), [], "an article is not a file");
});

test("Pinterest, Imgur, Reddit, Flickr", () => {
  assert.deepEqual(candidates("https://i.pinimg.com/236x/1a/2b/3c/abc.jpg"), [
    "https://i.pinimg.com/originals/1a/2b/3c/abc.jpg",
    "https://i.pinimg.com/736x/1a/2b/3c/abc.jpg",
  ]);
  assert.equal(first("https://i.imgur.com/AbCdEfGm.jpg"), "https://i.imgur.com/AbCdEfG.jpg");
  assert.deepEqual(candidates("https://i.imgur.com/AbCdEfG.jpg"), [], "seven letters: already the original");
  assert.equal(first("https://preview.redd.it/xyz123.jpg?width=640&crop=smart&auto=webp&s=abc"), "https://i.redd.it/xyz123.jpg");
  assert.equal(first("https://live.staticflickr.com/65535/123_abcdef_m.jpg"), "https://live.staticflickr.com/65535/123_abcdef_b.jpg");
});

test("CDNs that resize by address", () => {
  assert.equal(first("https://example.com/wp-content/uploads/2024/05/foto-300x200.jpg"), "https://example.com/wp-content/uploads/2024/05/foto.jpg");
  assert.equal(first("https://i0.wp.com/example.com/a.jpg?resize=300%2C200&ssl=1"), "https://i0.wp.com/example.com/a.jpg");
  assert.equal(first("https://cdn.shopify.com/s/files/1/0/products/tenis_200x200.jpg?v=1"), "https://cdn.shopify.com/s/files/1/0/products/tenis.jpg?v=1");
  assert.equal(first("https://m.media-amazon.com/images/I/71abc._AC_SX300_.jpg"), "https://m.media-amazon.com/images/I/71abc.jpg");
  assert.equal(first("https://i.ebayimg.com/images/g/abc/s-l300.jpg"), "https://i.ebayimg.com/images/g/abc/s-l1600.jpg");
  assert.equal(first("https://lh3.googleusercontent.com/abc=w400-h300-c"), "https://lh3.googleusercontent.com/abc=s0");
  assert.equal(first("https://images.unsplash.com/photo-1?w=400&q=80&ixid=abc"), "https://images.unsplash.com/photo-1?ixid=abc");
  assert.equal(first("https://res.cloudinary.com/demo/image/upload/w_300,c_fill/v1/sample.jpg"), "https://res.cloudinary.com/demo/image/upload/v1/sample.jpg");
  assert.equal(
    first("https://static.wixstatic.com/media/abc~mv2.jpg/v1/fill/w_300,h_200/abc~mv2.jpg"),
    "https://static.wixstatic.com/media/abc~mv2.jpg",
  );
  assert.equal(first("https://i.ytimg.com/vi/jNQXAC9IVRw/hqdefault.jpg"), "https://i.ytimg.com/vi/jNQXAC9IVRw/maxresdefault.jpg");
  assert.equal(first("https://i.ytimg.com/vi/dxsTUMwd09c/hq720.jpg?sqp=-oay&rs=AOn4"), "https://i.ytimg.com/vi/dxsTUMwd09c/maxresdefault.jpg");
  assert.deepEqual(candidates("https://i.ytimg.com/vi/dxsTUMwd09c/maxresdefault.jpg"), [], "already the largest");
});

test("signed or unknown addresses are left alone", () => {
  assert.deepEqual(candidates("https://scontent.cdninstagram.com/v/t51/123_n.jpg?stp=dst-jpg_e35&oh=abc&oe=def"), []);
  assert.deepEqual(candidates("https://example.com/foto.jpg"), []);
  assert.deepEqual(candidates("data:image/png;base64,AAAA"), []);
  assert.deepEqual(candidates("not a url"), []);
});

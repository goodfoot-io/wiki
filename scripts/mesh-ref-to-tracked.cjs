#!/usr/bin/env node
"use strict";

// scripts/mesh-ref-to-tracked-file.mjs
var import_node_child_process = require("node:child_process");
var import_node_util = require("node:util");
var import_node_crypto = require("node:crypto");
var import_node_fs = require("node:fs");
var import_node_path = require("node:path");

// node_modules/rkyv-js/src/reader.ts
var RkyvReader = class {
  view;
  buffer;
  textDecoder;
  constructor(buffer, options = {}) {
    this.textDecoder = options.textDecoder || new TextDecoder();
    if (buffer instanceof Uint8Array) {
      this.buffer = buffer;
      this.view = new DataView(buffer.buffer, buffer.byteOffset, buffer.byteLength);
    } else {
      this.buffer = new Uint8Array(buffer);
      this.view = new DataView(buffer);
    }
  }
  get length() {
    return this.buffer.length;
  }
  /**
   * Get the root position for rkyv archives.
   * rkyv lays out objects depth-first from leaves to root,
   * meaning the root object is at the end of the buffer.
   */
  getRootPosition(rootSize) {
    return this.length - rootSize;
  }
  // === Primitive Type Readers (Little Endian) ===
  readU8(offset) {
    return this.view.getUint8(offset);
  }
  readI8(offset) {
    return this.view.getInt8(offset);
  }
  readU16(offset) {
    return this.view.getUint16(offset, true);
  }
  readI16(offset) {
    return this.view.getInt16(offset, true);
  }
  readU32(offset) {
    return this.view.getUint32(offset, true);
  }
  readI32(offset) {
    return this.view.getInt32(offset, true);
  }
  readU64(offset) {
    return this.view.getBigUint64(offset, true);
  }
  readI64(offset) {
    return this.view.getBigInt64(offset, true);
  }
  readF32(offset) {
    return this.view.getFloat32(offset, true);
  }
  readF64(offset) {
    return this.view.getFloat64(offset, true);
  }
  readBool(offset) {
    return this.view.getUint8(offset) !== 0;
  }
  /**
   * Read a raw byte slice from the buffer
   */
  readBytes(offset, length) {
    return this.buffer.subarray(offset, offset + length);
  }
  /**
   */
  readText(offset, length) {
    const bytes = this.readBytes(offset, length);
    return this.textDecoder.decode(bytes);
  }
  /**
   * Read a relative pointer (32-bit signed offset by default in rkyv).
   * Returns the absolute position the pointer points to.
   *
   * @param offset - The position of the relative pointer in the buffer
   * @returns The absolute position the pointer points to
   */
  readRelPtr32(offset) {
    const relativeOffset = this.view.getInt32(offset, true);
    return offset + relativeOffset;
  }
  /**
   * Read a 16-bit relative pointer.
   */
  readRelPtr16(offset) {
    const relativeOffset = this.view.getInt16(offset, true);
    return offset + relativeOffset;
  }
  /**
   * Read a 64-bit relative pointer.
   */
  readRelPtr64(offset) {
    const relativeOffset = this.view.getBigInt64(offset, true);
    return BigInt(offset) + relativeOffset;
  }
};

// node_modules/rkyv-js/src/codec.ts
function alignOffset(offset, align) {
  const remainder = offset % align;
  return remainder === 0 ? offset : offset + (align - remainder);
}
function decode(codec, bytes) {
  const reader = new RkyvReader(bytes);
  const rootPosition = reader.getRootPosition(codec.size);
  return codec.decode(reader, rootPosition);
}

// node_modules/rkyv-js/src/primitives.ts
function createPrimitiveCodec(size, align, read, write) {
  return {
    size,
    align,
    access: read,
    // For primitives, access === decode
    decode: read,
    _archive: () => ({ pos: 0 }),
    _resolve: (writer, value) => write(writer, value),
    encode: (writer, value) => write(writer, value)
  };
}
var u8 = createPrimitiveCodec(1, 1, (r, o) => r.readU8(o), (w, v) => w.writeU8(v));
var i8 = createPrimitiveCodec(1, 1, (r, o) => r.readI8(o), (w, v) => w.writeI8(v));
var u16 = createPrimitiveCodec(2, 2, (r, o) => r.readU16(o), (w, v) => w.writeU16(v));
var i16 = createPrimitiveCodec(2, 2, (r, o) => r.readI16(o), (w, v) => w.writeI16(v));
var u32 = createPrimitiveCodec(4, 4, (r, o) => r.readU32(o), (w, v) => w.writeU32(v));
var i32 = createPrimitiveCodec(4, 4, (r, o) => r.readI32(o), (w, v) => w.writeI32(v));
var u64 = createPrimitiveCodec(8, 8, (r, o) => r.readU64(o), (w, v) => w.writeU64(v));
var i64 = createPrimitiveCodec(8, 8, (r, o) => r.readI64(o), (w, v) => w.writeI64(v));
var f32 = createPrimitiveCodec(4, 4, (r, o) => r.readF32(o), (w, v) => w.writeF32(v));
var f64 = createPrimitiveCodec(8, 8, (r, o) => r.readF64(o), (w, v) => w.writeF64(v));
var bool = createPrimitiveCodec(1, 1, (r, o) => r.readBool(o), (w, v) => w.writeBool(v));
var unit = {
  size: 0,
  align: 1,
  _archive: () => ({ pos: 0 }),
  _resolve: (writer) => writer.pos,
  access: () => null,
  decode: () => null,
  encode: (writer) => writer.pos
};
function decodeString(reader, offset) {
  const firstByte = reader.readU8(offset);
  const isInline = (firstByte & 192) !== 128;
  if (isInline) {
    let length = 0;
    while (length < 8) {
      if (reader.readU8(offset + length) === 255) break;
      length++;
    }
    return reader.readText(offset, length);
  } else {
    const lenWithMarker = reader.readU32(offset);
    const length = lenWithMarker & 63 | lenWithMarker >>> 8 << 6;
    const relPtr = reader.readI32(offset + 4);
    return reader.readText(offset + relPtr, length);
  }
}
var string = {
  size: 8,
  align: 4,
  access: decodeString,
  // Strings are immutable, no benefit from lazy access
  decode: decodeString,
  _archive(writer, value) {
    const bytes = writer.encodeText(value);
    if (bytes.length <= 8) {
      return { pos: 0, len: bytes.length };
    } else {
      const pos = writer.writeBytes(bytes);
      return { pos, len: bytes.length };
    }
  },
  _resolve(writer, value, resolver) {
    writer.align(4);
    const structPos = writer.pos;
    const bytes = writer.encodeText(value);
    if (bytes.length <= 8) {
      for (let i = 0; i < bytes.length; i++) writer.writeU8(bytes[i]);
      for (let i = bytes.length; i < 8; i++) writer.writeU8(255);
    } else {
      const len = bytes.length;
      const encodedLen = len & 63 | 128 | (len & ~63) << 2;
      writer.writeU32(encodedLen);
      writer.writeI32(resolver.pos - structPos);
    }
    return structPos;
  },
  encode(writer, value) {
    const resolver = this._archive(writer, value);
    return this._resolve(writer, value, resolver);
  }
};
function vec(element) {
  let _elementStride = null;
  const getElementStride = () => {
    if (_elementStride === null) {
      _elementStride = alignOffset(element.size, element.align);
    }
    return _elementStride;
  };
  return {
    size: 8,
    // relptr (4) + len (4)
    align: 4,
    // Zero-copy access: returns a Proxy that lazily decodes elements
    access(reader, offset) {
      const dataOffset = reader.readRelPtr32(offset);
      const length = reader.readU32(offset + 4);
      const cache = /* @__PURE__ */ new Map();
      const elementStride = getElementStride();
      return new Proxy([], {
        get(target, prop) {
          if (prop === "length") return length;
          if (prop === Symbol.iterator) {
            return function* () {
              for (let i = 0; i < length; i++) {
                if (!cache.has(i)) {
                  const elemOffset = dataOffset + i * elementStride;
                  cache.set(i, element.access(reader, elemOffset));
                }
                yield cache.get(i);
              }
            };
          }
          if (typeof prop === "string") {
            const index = parseInt(prop, 10);
            if (!isNaN(index) && index >= 0 && index < length) {
              if (!cache.has(index)) {
                const elemOffset = dataOffset + index * elementStride;
                cache.set(index, element.access(reader, elemOffset));
              }
              return cache.get(index);
            }
          }
          return Reflect.get(target, prop);
        },
        has(target, prop) {
          if (typeof prop === "string") {
            const index = parseInt(prop, 10);
            if (!isNaN(index)) return index >= 0 && index < length;
          }
          return Reflect.has(target, prop);
        },
        ownKeys() {
          return [...Array(length).keys()].map(String);
        },
        getOwnPropertyDescriptor(target, prop) {
          if (typeof prop === "string") {
            const index = parseInt(prop, 10);
            if (!isNaN(index) && index >= 0 && index < length) {
              return { enumerable: true, configurable: true, writable: false };
            }
          }
          return Reflect.getOwnPropertyDescriptor(target, prop);
        }
      });
    },
    decode(reader, offset) {
      const dataOffset = reader.readRelPtr32(offset);
      const length = reader.readU32(offset + 4);
      const result = new Array(length);
      let currentOffset = dataOffset;
      for (let i = 0; i < length; i++) {
        currentOffset = alignOffset(currentOffset, element.align);
        result[i] = element.decode(reader, currentOffset);
        currentOffset += element.size;
      }
      return result;
    },
    _archive(writer, value) {
      if (value.length === 0) {
        return { pos: writer.pos, len: 0, elementResolvers: [] };
      }
      const elementResolvers = [];
      for (const item of value) {
        elementResolvers.push(element._archive(writer, item));
      }
      writer.align(element.align);
      const elementsStartPos = writer.pos;
      for (let i = 0; i < value.length; i++) {
        writer.align(element.align);
        element._resolve(writer, value[i], elementResolvers[i]);
      }
      return { pos: elementsStartPos, len: value.length, elementResolvers };
    },
    _resolve(writer, _value, resolver) {
      writer.align(4);
      const structPos = writer.pos;
      const ptrPos = writer.reserveRelPtr32();
      const r = resolver;
      writer.writeU32(r.len);
      if (r.len > 0) {
        writer.writeRelPtr32At(ptrPos, r.pos);
      } else {
        writer.writeRelPtr32At(ptrPos, 0);
      }
      return structPos;
    },
    encode(writer, value) {
      const resolver = this._archive(writer, value);
      return this._resolve(writer, value, resolver);
    }
  };
}
function tuple(...codecs) {
  let currentSize = 0;
  let maxAlign = 1;
  const offsets = [];
  for (const codec of codecs) {
    currentSize = alignOffset(currentSize, codec.align);
    offsets.push(currentSize);
    currentSize += codec.size;
    maxAlign = Math.max(maxAlign, codec.align);
  }
  const totalSize = alignOffset(currentSize, maxAlign);
  return {
    size: totalSize,
    align: maxAlign,
    // Tuple access with lazy element decoding
    access(reader, offset) {
      const cache = /* @__PURE__ */ new Map();
      const length = codecs.length;
      return new Proxy([], {
        get(target, prop) {
          if (prop === "length") return length;
          if (typeof prop === "string") {
            const index = parseInt(prop, 10);
            if (!isNaN(index) && index >= 0 && index < length) {
              if (!cache.has(index)) {
                cache.set(index, codecs[index].access(reader, offset + offsets[index]));
              }
              return cache.get(index);
            }
          }
          return Reflect.get(target, prop);
        }
      });
    },
    decode(reader, offset) {
      const result = new Array(codecs.length);
      for (let i = 0; i < codecs.length; i++) {
        result[i] = codecs[i].decode(reader, offset + offsets[i]);
      }
      return result;
    },
    _archive(writer, value) {
      const resolvers = [];
      for (let i = 0; i < codecs.length; i++) {
        resolvers.push(codecs[i]._archive(writer, value[i]));
      }
      return { pos: writer.pos, resolvers };
    },
    _resolve(writer, value, resolver) {
      writer.align(this.align);
      const pos = writer.pos;
      const resolvers = resolver.resolvers;
      for (let i = 0; i < codecs.length; i++) {
        writer.align(codecs[i].align);
        codecs[i]._resolve(writer, value[i], resolvers[i]);
      }
      writer.padTo(pos + totalSize);
      return pos;
    },
    encode(writer, value) {
      const resolver = this._archive(writer, value);
      return this._resolve(writer, value, resolver);
    }
  };
}
function struct(fields) {
  const fieldNames = Object.keys(fields);
  const fieldCodecs = fieldNames.map((name) => ({ name, codec: fields[name] }));
  let currentOffset = 0;
  let maxAlign = 1;
  const fieldOffsets = /* @__PURE__ */ new Map();
  for (const { name, codec } of fieldCodecs) {
    currentOffset = alignOffset(currentOffset, codec.align);
    fieldOffsets.set(name, currentOffset);
    currentOffset += codec.size;
    maxAlign = Math.max(maxAlign, codec.align);
  }
  const totalSize = alignOffset(currentOffset, maxAlign);
  return {
    size: totalSize,
    align: maxAlign,
    /**
     * Zero-copy access: Returns a Proxy that lazily decodes fields on demand.
     * Fields are cached after first access for repeated reads.
     */
    access(reader, offset) {
      const cache = /* @__PURE__ */ new Map();
      const fieldMap = new Map(fieldCodecs.map((f) => [f.name, f]));
      return new Proxy({}, {
        get(target, prop) {
          const name = prop;
          if (fieldMap.has(prop)) {
            if (!cache.has(name)) {
              const { codec } = fieldMap.get(prop);
              const fieldOffset = fieldOffsets.get(name);
              cache.set(name, codec.access(reader, offset + fieldOffset));
            }
            return cache.get(name);
          }
          return Reflect.get(target, prop);
        },
        has(target, prop) {
          if (fieldMap.has(prop)) return true;
          return Reflect.has(target, prop);
        },
        ownKeys() {
          return fieldNames;
        },
        getOwnPropertyDescriptor(target, prop) {
          if (fieldMap.has(prop)) {
            return { enumerable: true, configurable: true, writable: false };
          }
          return Reflect.getOwnPropertyDescriptor(target, prop);
        }
      });
    },
    decode(reader, offset) {
      const result = {};
      for (const { name, codec } of fieldCodecs) {
        result[name] = codec.decode(reader, offset + fieldOffsets.get(name));
      }
      return result;
    },
    _archive(writer, value) {
      const fieldResolvers = /* @__PURE__ */ new Map();
      for (const { name, codec } of fieldCodecs) {
        fieldResolvers.set(name, codec._archive(writer, value[name]));
      }
      return { pos: writer.pos, fieldResolvers };
    },
    _resolve(writer, value, resolver) {
      writer.align(this.align);
      const pos = writer.pos;
      const fieldResolvers = resolver.fieldResolvers;
      for (const { name, codec } of fieldCodecs) {
        writer.padTo(pos + fieldOffsets.get(name));
        codec._resolve(writer, value[name], fieldResolvers.get(name));
      }
      writer.padTo(pos + totalSize);
      return pos;
    },
    encode(writer, value) {
      const resolver = this._archive(writer, value);
      return this._resolve(writer, value, resolver);
    }
  };
}
function taggedEnum(variants) {
  const variantNames = Object.keys(variants);
  const variantCount = variantNames.length;
  let discriminantSize;
  if (variantCount <= 256) discriminantSize = 1;
  else if (variantCount <= 65536) discriminantSize = 2;
  else discriminantSize = 4;
  const variantInfo = /* @__PURE__ */ new Map();
  let maxVariantAlign = discriminantSize;
  let maxVariantSize = 0;
  variantNames.forEach((name, index) => {
    const codec = variants[name];
    const isUnit = codec.size === 0;
    variantInfo.set(name, { index, codec, isUnit });
    if (!isUnit) {
      maxVariantAlign = Math.max(maxVariantAlign, codec.align);
      maxVariantSize = Math.max(maxVariantSize, codec.size);
    }
  });
  const maxDiscriminantPadding = alignOffset(discriminantSize, maxVariantAlign) - discriminantSize;
  const totalSize = discriminantSize + maxDiscriminantPadding + maxVariantSize;
  return {
    size: totalSize,
    align: maxVariantAlign,
    access(reader, offset) {
      let discriminant;
      if (discriminantSize === 1) discriminant = reader.readU8(offset);
      else if (discriminantSize === 2) discriminant = reader.readU16(offset);
      else discriminant = reader.readU32(offset);
      const tag = variantNames[discriminant];
      const info = variantInfo.get(tag);
      if (info.isUnit) {
        return { tag, value: null };
      }
      const variantPadding = alignOffset(discriminantSize, info.codec.align) - discriminantSize;
      const valueOffset = offset + discriminantSize + variantPadding;
      const value = info.codec.access(reader, valueOffset);
      return { tag, value };
    },
    decode(reader, offset) {
      let discriminant;
      if (discriminantSize === 1) discriminant = reader.readU8(offset);
      else if (discriminantSize === 2) discriminant = reader.readU16(offset);
      else discriminant = reader.readU32(offset);
      const tag = variantNames[discriminant];
      const info = variantInfo.get(tag);
      if (info.isUnit) {
        return { tag, value: null };
      }
      const variantPadding = alignOffset(discriminantSize, info.codec.align) - discriminantSize;
      const valueOffset = offset + discriminantSize + variantPadding;
      const value = info.codec.decode(reader, valueOffset);
      return { tag, value };
    },
    _archive(writer, value) {
      const enumValue = value;
      const info = variantInfo.get(enumValue.tag);
      if (info.isUnit) {
        return { pos: writer.pos, variantResolver: null };
      }
      return { pos: writer.pos, variantResolver: info.codec._archive(writer, enumValue.value) };
    },
    _resolve(writer, value, resolver) {
      writer.align(this.align);
      const pos = writer.pos;
      const enumValue = value;
      const info = variantInfo.get(enumValue.tag);
      if (discriminantSize === 1) writer.writeU8(info.index);
      else if (discriminantSize === 2) writer.writeU16(info.index);
      else writer.writeU32(info.index);
      if (!info.isUnit) {
        const variantPadding = alignOffset(discriminantSize, info.codec.align) - discriminantSize;
        for (let i = 0; i < variantPadding; i++) writer.writeU8(0);
        const variantResolver = resolver.variantResolver;
        info.codec._resolve(writer, enumValue.value, variantResolver);
      }
      writer.padTo(pos + totalSize);
      return pos;
    },
    encode(writer, value) {
      const resolver = this._archive(writer, value);
      return this._resolve(writer, value, resolver);
    }
  };
}

// scripts/mesh-ref-to-tracked-file.mjs
var AnchorExtent = taggedEnum({
  WholeFile: unit,
  LineRange: struct({ start: u32, end: u32 })
});
var AnchorEntry = struct({
  anchor_sha: string,
  created_at: string,
  path: string,
  extent: AnchorExtent,
  blob: string
});
var CopyDetection = taggedEnum({
  Off: unit,
  SameCommit: unit,
  AnyFileInCommit: unit,
  AnyFileInRepo: unit
});
var MeshConfig = struct({
  copy_detection: CopyDetection,
  ignore_whitespace: bool,
  follow_moves: bool
});
var MeshArchive = struct({
  name: string,
  anchors_v2: vec(tuple(string, AnchorEntry)),
  message: string,
  config: MeshConfig
});
var FORMAT_VERSION = 0;
var HEADER_LEN = 8;
var { values } = (0, import_node_util.parseArgs)({
  options: {
    "dry-run": { type: "boolean", default: false },
    "mesh-dir": { type: "string" },
    "prune-refs": { type: "boolean", default: false }
  }
});
var dryRun = values["dry-run"];
function git(args, { buffer = false, input } = {}) {
  return (0, import_node_child_process.execFileSync)("git", args, {
    input,
    encoding: buffer ? "buffer" : "utf8",
    maxBuffer: 512 * 1024 * 1024
  });
}
function gitTry(args) {
  try {
    return git(args).trim();
  } catch {
    return null;
  }
}
var repoRoot = git(["rev-parse", "--show-toplevel"]).trim();
function resolveMeshRoot() {
  const candidate = values["mesh-dir"] ?? process.env.GIT_MESH_DIR ?? gitTry(["config", "git-mesh.dir"]) ?? ".mesh";
  if ((0, import_node_path.isAbsolute)(candidate)) {
    throw new Error(`mesh root must be repo-relative, got absolute: ${candidate}`);
  }
  const norm = (0, import_node_path.normalize)(candidate);
  if (norm === ".." || norm.startsWith("../") || norm.split("/").includes("..")) {
    throw new Error(`mesh root must not contain "..": ${candidate}`);
  }
  if (norm === ".git" || norm.startsWith(".git/")) {
    throw new Error(`mesh root must not live inside .git: ${candidate}`);
  }
  return norm;
}
var meshRoot = resolveMeshRoot();
function catalogMeshBlobs() {
  const commit = gitTry(["for-each-ref", "--format=%(objectname)", "refs/meshes/v1/catalog"]);
  if (!commit) return [];
  const tree = git(["ls-tree", "refs/meshes/v1/catalog^{tree}"]);
  const out = [];
  for (const line of tree.split("\n")) {
    if (!line) continue;
    const tab = line.indexOf("	");
    const entry = line.slice(tab + 1);
    const oid = line.slice(0, tab).split(/\s+/)[2];
    if (!entry.endsWith(".mesh")) continue;
    const name = entry.slice(0, -".mesh".length).split("++").join("/");
    out.push({ name, oid });
  }
  return out.sort((a, b) => a.name.localeCompare(b.name));
}
function decodeMesh(oid) {
  const buf = git(["cat-file", "blob", oid], { buffer: true });
  if (buf.length < HEADER_LEN || buf[0] !== FORMAT_VERSION) {
    throw new Error(`unexpected mesh blob header for ${oid} (byte0=${buf[0]})`);
  }
  const payload = buf.subarray(HEADER_LEN);
  return decode(MeshArchive, payload);
}
function extentSha256(blobOid, extent) {
  const buf = git(["cat-file", "blob", blobOid], { buffer: true });
  let slice;
  if (extent.tag === "WholeFile") {
    slice = buf;
  } else {
    const { start, end } = extent.value;
    let line = 1;
    let startByte = start === 1 ? 0 : null;
    let endByte = buf.length;
    for (let i = 0; i < buf.length; i++) {
      if (buf[i] === 10) {
        if (line === end) {
          endByte = i + 1;
          break;
        }
        line++;
        if (line === start) startByte = i + 1;
      }
    }
    if (startByte === null) startByte = buf.length;
    slice = buf.subarray(startByte, endByte);
  }
  return "sha256:" + (0, import_node_crypto.createHash)("sha256").update(slice).digest("hex");
}
function anchorAddress(a) {
  if (a.extent.tag === "WholeFile") return a.path;
  return `${a.path}#L${a.extent.value.start}-L${a.extent.value.end}`;
}
function renderMeshFile(mesh) {
  const lines = mesh.anchors_v2.map(([, a]) => `${anchorAddress(a)} ${extentSha256(a.blob, a.extent)}`);
  const why = mesh.message.replace(/\s+$/, "");
  return `${lines.join("\n")}

${why}
`;
}
var blobs = catalogMeshBlobs();
if (blobs.length === 0) {
  console.log("No legacy catalog found; nothing to migrate.");
}
var written = 0;
for (const { name, oid } of blobs) {
  let mesh;
  try {
    mesh = decodeMesh(oid);
  } catch (err) {
    console.warn(`[${name}] decode failed: ${err.message}; skipping`);
    continue;
  }
  if (!mesh.anchors_v2.length) {
    console.warn(`[${name}] no anchors; skipping`);
    continue;
  }
  const relPath = (0, import_node_path.join)(meshRoot, name);
  const absPath = (0, import_node_path.join)(repoRoot, relPath);
  const content = renderMeshFile(mesh);
  if (dryRun) {
    console.log(`--- [dry-run] would write ${relPath} ---`);
    console.log(content.replace(/\n$/, ""));
  } else {
    (0, import_node_fs.mkdirSync)((0, import_node_path.dirname)(absPath), { recursive: true });
    (0, import_node_fs.writeFileSync)(absPath, content, "utf8");
    console.log(`wrote ${relPath} (${mesh.anchors_v2.length} anchor(s))`);
  }
  written++;
}
console.log(
  `${dryRun ? "[dry-run] " : ""}${written}/${blobs.length} mesh file(s) under ${meshRoot}/`
);
console.log(
  dryRun ? "[dry-run] after a real run: git add the mesh root and commit." : `Next: git add ${meshRoot} && git commit -m "Migrate meshes to tracked files"`
);
if (values["prune-refs"]) {
  const namespaces = ["refs/meshes/v1/", "refs/anchors/v1/", "refs/meshes-index/v1/"];
  const refs = [];
  for (const ns of namespaces) {
    const o = gitTry(["for-each-ref", "--format=%(refname)", ns]) ?? "";
    for (const ref of o.split("\n")) if (ref) refs.push(ref);
  }
  if (refs.length === 0) {
    console.log("No legacy refs to prune.");
  } else if (dryRun) {
    console.log(`[dry-run] would delete ${refs.length} legacy ref(s):`);
    for (const ref of refs) console.log(`  ${ref}`);
  } else {
    git(["update-ref", "--stdin"], { input: refs.map((ref) => `delete ${ref}`).join("\n") + "\n" });
    console.log(`Deleted ${refs.length} legacy ref(s).`);
  }
}

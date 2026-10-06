// Bounded replacements for Boa 0.22 built-ins whose single call can amplify
// memory, iterate natively, or recurse natively far beyond the SCXML value
// bound. Boa 0.22 exposes no allocation hook, interrupt, or native-recursion
// limit, so every guard below rejects with a catchable RangeError *before* the
// native built-in allocates, iterates, or recurses.
//
// Contract:
// - Evaluates to a function that the adapter calls once per Boa context with
//   (MAX_VALUE, MAX_NESTING), after BOOTSTRAP and before any user
//   script, so no bound is a global name (SCXML 1.0 section 5.10 reserves
//   platform-specific variables to the `_x` root). Every intrinsic the guards use is captured here, so later user
//   code that overwrites globals (Math, Reflect, String, ...) cannot disable a
//   guard. Guarded properties are non-writable and non-configurable.
// - Array-like lengths are read once from a data property (or the native
//   typed-array getter); accessor or object-valued lengths are rejected so a
//   native built-in can never observe a different length than the guard did.
// - Bounds: result strings and array lengths <= MAX_VALUE (UTF-16 code units,
//   which never exceed UTF-8 bytes); nesting <= MAX_NESTING; auxiliary work
//   (template tokens, flatten steps) <= MAX_WORK. JSON.parse text is at most
//   MAX_VALUE code units with array/object nesting <= MAX_NESTING.
(function (MAX_VALUE, MAX_NESTING) {
  "use strict";
  // JSON output estimate cap: structure overhead is estimated per node, so the
  // estimate gets 2x headroom; escapes can still expand string content by at
  // most 6x, which keeps the worst case a small constant multiple of MAX_VALUE.
  const MAX_JSON = MAX_VALUE * 2;
  const MAX_WORK = MAX_VALUE * 16;
  const MAX_SAFE_LENGTH = 9007199254740991;

  const apply = Reflect.apply;
  const construct = Reflect.construct;
  const defineProperty = Object.defineProperty;
  const getOwnPropertyDescriptor = Object.getOwnPropertyDescriptor;
  const getPrototypeOf = Object.getPrototypeOf;
  const hasOwn = Object.hasOwn;
  const objectCreate = Object.create;
  const isArray = Array.isArray;
  const NativeArray = Array;
  const NativeObject = Object;
  const NativeRangeError = RangeError;
  const NativeTypeError = TypeError;
  const NativeWeakMap = WeakMap;
  const weakGet = WeakMap.prototype.get;
  const weakSet = WeakMap.prototype.set;
  const trunc = Math.trunc;
  const min = Math.min;
  const iteratorSymbol = Symbol.iterator;
  const replaceSymbol = Symbol.replace;
  const spreadableSymbol = Symbol.isConcatSpreadable;
  const stringProto = String.prototype;
  const arrayProto = Array.prototype;
  const nativeRepeat = stringProto.repeat;
  const nativePadStart = stringProto.padStart;
  const nativePadEnd = stringProto.padEnd;
  const nativeStringConcat = stringProto.concat;
  const nativeReplace = stringProto.replace;
  const nativeReplaceAll = stringProto.replaceAll;
  const nativeSlice = stringProto.slice;
  const nativeIndexOf = stringProto.indexOf;
  const nativeStringValueOf = stringProto.valueOf;
  const nativeNumberValueOf = Number.prototype.valueOf;
  const nativeBooleanValueOf = Boolean.prototype.valueOf;
  const nativeRegExpReplace = RegExp.prototype[replaceSymbol];
  const nativeJoin = arrayProto.join;
  const nativeStringify = JSON.stringify;
  const nativeParse = JSON.parse;
  const nativeCharCodeAt = stringProto.charCodeAt;
  const typedArrayLength = getOwnPropertyDescriptor(
    getPrototypeOf(Uint8Array.prototype), "length").get;
  const arrayIteratorPrototype = getPrototypeOf(apply(arrayProto.values, [], []));
  const nativeArrayIteratorNext = arrayIteratorPrototype.next;
  const localeSeparator = apply(nativeSlice,
    apply(arrayProto.toLocaleString, [0, 0], []), [1, -1]);

  const reject = (what) => {
    throw new NativeRangeError(what + " exceeds the SCXML data-model bound");
  };
  const lock = (target, key, value) => defineProperty(target, key, {
    value, writable: false, enumerable: false, configurable: false,
  });
  const append = (list, value) => defineProperty(list, list.length, {
    value, writable: true, enumerable: true, configurable: true,
  });
  const requireCoercible = (value, method) => {
    if (value === null || value === undefined) {
      throw new NativeTypeError(method + " called on null or undefined");
    }
    return value;
  };
  const toObject = (value, method) => NativeObject(requireCoercible(value, method));
  const toText = (value) => `${value}`;
  const toIntegerOrInfinity = (value) => {
    const number = +value;
    if (number !== number) return 0;
    return trunc(number) + 0;
  };
  const toLength = (value) => {
    const integer = toIntegerOrInfinity(value);
    return integer <= 0 ? 0 : min(integer, MAX_SAFE_LENGTH);
  };
  const isObject = (value) =>
    (typeof value === "object" && value !== null) || typeof value === "function";

  // LengthOfArrayLike, read exactly once from a data property.
  const lengthOf = (object) => {
    if (isArray(object)) return object.length;
    let holder = NativeObject(object);
    let descriptor;
    let steps = 0;
    while (holder !== null) {
      steps += 1;
      if (steps > MAX_NESTING) reject("array-like prototype depth");
      descriptor = getOwnPropertyDescriptor(holder, "length");
      if (descriptor !== undefined) break;
      holder = getPrototypeOf(holder);
    }
    if (descriptor === undefined) return 0;
    if (!hasOwn(descriptor, "value")) {
      if (descriptor.get === typedArrayLength) {
        return apply(typedArrayLength, object, []);
      }
      reject("array-like accessor length");
    }
    if (isObject(descriptor.value)) reject("array-like object length");
    return toLength(descriptor.value);
  };
  const boundedLength = (object, what) => {
    const length = lengthOf(object);
    if (length > MAX_VALUE) reject(what);
    return length;
  };
  const withLength = (guarded, native) => {
    defineProperty(guarded, "length", { value: native.length, configurable: true });
    return guarded;
  };

  // ---- Strings -----------------------------------------------------------

  lock(stringProto, "repeat", withLength({ repeat(count) {
    const string = toText(requireCoercible(this, "String.prototype.repeat"));
    const times = toIntegerOrInfinity(count);
    if (times >= 0 && times !== Infinity && string.length !== 0 &&
        times > MAX_VALUE / string.length) {
      reject("repeated string");
    }
    return apply(nativeRepeat, string, [times]);
  } }.repeat, nativeRepeat));

  const pad = (native, receiver, maxLength, fillString, method) => {
    const string = toText(requireCoercible(receiver, method));
    const target = toLength(maxLength);
    if (target <= string.length) return string;
    const filler = fillString === undefined ? " " : toText(fillString);
    if (filler !== "" && target > MAX_VALUE) reject("padded string");
    return apply(native, string, [target, filler]);
  };
  lock(stringProto, "padStart", withLength({ padStart(maxLength) {
    return pad(nativePadStart, this, maxLength, arguments[1], "String.prototype.padStart");
  } }.padStart, nativePadStart));
  lock(stringProto, "padEnd", withLength({ padEnd(maxLength) {
    return pad(nativePadEnd, this, maxLength, arguments[1], "String.prototype.padEnd");
  } }.padEnd, nativePadEnd));

  lock(stringProto, "concat", withLength({ concat(first) {
    const string = toText(requireCoercible(this, "String.prototype.concat"));
    let total = string.length;
    const parts = [];
    for (let index = 0; index < arguments.length; index += 1) {
      const part = toText(arguments[index]);
      total += part.length;
      if (total > MAX_VALUE) reject("concatenated string");
      append(parts, part);
    }
    return apply(nativeStringConcat, string, parts);
  } }.concat, nativeStringConcat));

  // A counting replacer for a callable replaceValue: the exact result length is
  // tracked as subject - matched + replacement per match.
  const countingReplacer = (replacer, subjectLength) => {
    let total = subjectLength;
    return function (matched) {
      const replacement = toText(apply(replacer, undefined, arguments));
      total = total - matched.length + replacement.length;
      if (total > MAX_VALUE) reject("replaced string");
      return replacement;
    };
  };

  // GetSubstitution (ECMA-262 22.1.3.19.1) as a functional replacer, so each
  // substitution is measured before it is materialized.
  const substitutingReplacer = (template, subjectLength) => {
    let total = subjectLength;
    let work = 0;
    const size = template.length;
    return function (matched) {
      let stringIndex = arguments.length - 1;
      let groups;
      if (typeof arguments[stringIndex] !== "string") {
        groups = arguments[stringIndex];
        stringIndex -= 1;
      }
      const string = arguments[stringIndex];
      const position = arguments[stringIndex - 1];
      const captureCount = stringIndex - 2;
      const captures = arguments;
      const pieces = [];
      total -= matched.length;
      const add = (piece) => {
        total += piece.length;
        if (total > MAX_VALUE) reject("replaced string");
        append(pieces, piece);
      };
      let index = 0;
      while (index < size) {
        work += 1;
        if (work > MAX_WORK) reject("replacement template work");
        const dollar = apply(nativeIndexOf, template, ["$", index]);
        if (dollar < 0) {
          add(apply(nativeSlice, template, [index]));
          break;
        }
        if (dollar > index) add(apply(nativeSlice, template, [index, dollar]));
        index = dollar;
        const next = index + 1 < size ? template[index + 1] : "";
        if (next === "$") {
          add("$");
          index += 2;
        } else if (next === "&") {
          add(matched);
          index += 2;
        } else if (next === "`") {
          add(apply(nativeSlice, string, [0, position]));
          index += 2;
        } else if (next === "'") {
          add(apply(nativeSlice, string, [min(position + matched.length, string.length)]));
          index += 2;
        } else if (next >= "0" && next <= "9") {
          const second = index + 2 < size ? template[index + 2] : "";
          let digits = second >= "0" && second <= "9" ? 2 : 1;
          let reference = digits === 2 ? +next * 10 + +second : +next;
          if (reference > captureCount && digits === 2) {
            digits = 1;
            reference = +next;
          }
          if (reference >= 1 && reference <= captureCount) {
            const capture = captures[reference];
            if (capture !== undefined) add(capture);
          } else {
            add(apply(nativeSlice, template, [index, index + 1 + digits]));
          }
          index += 1 + digits;
        } else if (next === "<") {
          const close = groups === undefined
            ? -1 : apply(nativeIndexOf, template, [">", index + 2]);
          if (close < 0) {
            add("$<");
            index += 2;
          } else {
            const capture = groups[apply(nativeSlice, template, [index + 2, close])];
            if (capture !== undefined) add(toText(capture));
            index = close + 1;
          }
        } else {
          add("$");
          index += 1;
        }
      }
      return apply(nativeJoin, pieces, [""]);
    };
  };
  const boundedReplacer = (replaceValue, subjectLength) =>
    typeof replaceValue === "function"
      ? countingReplacer(replaceValue, subjectLength)
      : substitutingReplacer(toText(replaceValue), subjectLength);

  lock(stringProto, "replace", withLength({ replace(searchValue, replaceValue) {
    const string = toText(requireCoercible(this, "String.prototype.replace"));
    return apply(nativeReplace, string,
      [searchValue, boundedReplacer(replaceValue, string.length)]);
  } }.replace, nativeReplace));
  lock(stringProto, "replaceAll", withLength({ replaceAll(searchValue, replaceValue) {
    const string = toText(requireCoercible(this, "String.prototype.replaceAll"));
    return apply(nativeReplaceAll, string,
      [searchValue, boundedReplacer(replaceValue, string.length)]);
  } }.replaceAll, nativeReplaceAll));
  lock(RegExp.prototype, replaceSymbol, withLength({ [replaceSymbol](string, replaceValue) {
    const subject = toText(string);
    return apply(nativeRegExpReplace, this,
      [subject, boundedReplacer(replaceValue, subject.length)]);
  } }[replaceSymbol], nativeRegExpReplace));

  lock(String, "raw", withLength({ raw(template) {
    const cooked = toObject(template, "String.raw");
    const literals = toObject(cooked.raw, "String.raw");
    const count = boundedLength(literals, "String.raw segment count");
    if (count === 0) return "";
    const pieces = [];
    let total = 0;
    for (let index = 0; ; index += 1) {
      const literal = toText(literals[index]);
      total += literal.length;
      if (total > MAX_VALUE) reject("raw string");
      append(pieces, literal);
      if (index + 1 === count) break;
      if (index + 1 < arguments.length) {
        const substitution = toText(arguments[index + 1]);
        total += substitution.length;
        if (total > MAX_VALUE) reject("raw string");
        append(pieces, substitution);
      }
    }
    return apply(nativeJoin, pieces, [""]);
  } }.raw, String.raw));

  // ---- Arrays ------------------------------------------------------------

  // join/toString/toLocaleString recurse through element conversion. The
  // bodies are kept inline (one script frame per nesting level) so the
  // catchable nesting rejection fires well before Boa's uncatchable
  // recursion limit.
  let joinNesting = 0;
  lock(arrayProto, "join", withLength({ join(separator) {
    const object = toObject(this, "Array.prototype.join");
    const length = boundedLength(object, "joined array length");
    const glue = separator === undefined ? "," : `${separator}`;
    if (length === 0) return "";
    joinNesting += 1;
    try {
      if (joinNesting > MAX_NESTING) reject("joined array nesting");
      const parts = [];
      let total = 0;
      for (let index = 0; index < length; index += 1) {
        const element = object[index];
        const text = element === undefined || element === null ? "" : `${element}`;
        total += text.length + (index === 0 ? 0 : glue.length);
        if (total > MAX_VALUE) reject("joined string");
        append(parts, text);
      }
      return apply(nativeJoin, parts, [glue]);
    } finally {
      joinNesting -= 1;
    }
  } }.join, arrayProto.join));
  lock(arrayProto, "toLocaleString", withLength({ toLocaleString() {
    const object = toObject(this, "Array.prototype.toLocaleString");
    const length = boundedLength(object, "joined array length");
    joinNesting += 1;
    try {
      if (joinNesting > MAX_NESTING) reject("joined array nesting");
      const parts = [];
      let total = 0;
      for (let index = 0; index < length; index += 1) {
        const element = object[index];
        const text = element === undefined || element === null
          ? "" : `${apply(element.toLocaleString, element, [])}`;
        total += text.length + (index === 0 ? 0 : localeSeparator.length);
        if (total > MAX_VALUE) reject("joined string");
        append(parts, text);
      }
      return apply(nativeJoin, parts, [localeSeparator]);
    } finally {
      joinNesting -= 1;
    }
  } }.toLocaleString, arrayProto.toLocaleString));

  const defineElement = (target, index, value) => {
    if (index >= MAX_VALUE) reject("array length");
    defineProperty(target, index, {
      value, writable: true, enumerable: true, configurable: true,
    });
  };

  lock(arrayProto, "concat", withLength({ concat(first) {
    const object = toObject(this, "Array.prototype.concat");
    const result = [];
    let count = 0;
    for (let item = -1; item < arguments.length; item += 1) {
      const element = item < 0 ? object : arguments[item];
      let spreadable = false;
      if (isObject(element)) {
        const flag = element[spreadableSymbol];
        spreadable = flag === undefined ? isArray(element) : !!flag;
      }
      if (spreadable) {
        const length = boundedLength(element, "concatenated array length");
        if (count + length > MAX_VALUE) reject("concatenated array length");
        for (let index = 0; index < length; index += 1) {
          if (index in element) defineElement(result, count, element[index]);
          count += 1;
        }
      } else {
        defineElement(result, count, element);
        count += 1;
      }
    }
    result.length = count;
    return result;
  } }.concat, arrayProto.concat));

  // FlattenIntoArray (ECMA-262 23.1.3.13.1) with explicit nesting, output and
  // work bounds. The JS recursion replaces Boa's native recursion.
  const flatten = (state, source, sourceLength, depth, level, mapper, thisArg) => {
    if (level > MAX_NESTING) reject("flattened array nesting");
    for (let index = 0; index < sourceLength; index += 1) {
      state.work += 1;
      if (state.work > MAX_WORK) reject("flattened array work");
      if (!(index in source)) continue;
      let element = source[index];
      if (mapper !== undefined) element = apply(mapper, thisArg, [element, index, source]);
      if (depth > 0 && isArray(element)) {
        flatten(state, element, boundedLength(element, "flattened array length"),
          depth === Infinity ? Infinity : depth - 1, level + 1, undefined, undefined);
      } else {
        defineElement(state.target, state.count, element);
        state.count += 1;
      }
    }
  };
  lock(arrayProto, "flat", withLength({ flat() {
    const object = toObject(this, "Array.prototype.flat");
    const length = boundedLength(object, "flattened array length");
    const depthArgument = arguments[0];
    let depth = depthArgument === undefined ? 1 : toIntegerOrInfinity(depthArgument);
    if (depth < 0) depth = 0;
    const state = { target: [], count: 0, work: 0 };
    flatten(state, object, length, depth, 1, undefined, undefined);
    return state.target;
  } }.flat, arrayProto.flat));
  lock(arrayProto, "flatMap", withLength({ flatMap(mapper) {
    const object = toObject(this, "Array.prototype.flatMap");
    const length = boundedLength(object, "flattened array length");
    if (typeof mapper !== "function") {
      throw new NativeTypeError("Array.prototype.flatMap mapper is not callable");
    }
    const state = { target: [], count: 0, work: 0 };
    flatten(state, object, length, 1, 1, mapper, arguments[1]);
    return state.target;
  } }.flatMap, arrayProto.flatMap));

  // Methods whose native implementation reads the length once and then
  // iterates or allocates proportionally to it.
  const lengthBounded = [
    "copyWithin", "entries", "every", "fill", "filter", "find", "findIndex",
    "findLast", "findLastIndex", "forEach", "includes", "indexOf", "keys",
    "lastIndexOf", "map", "reduce", "reduceRight", "reverse", "shift", "slice",
    "some", "sort", "splice", "toReversed", "toSorted", "toSpliced", "unshift",
    "values", "with",
  ];
  for (let position = 0; position < lengthBounded.length; position += 1) {
    const name = lengthBounded[position];
    const native = arrayProto[name];
    if (typeof native !== "function") continue;
    const guarded = withLength({ [name]() {
      if (this !== null && this !== undefined) boundedLength(this, "array length");
      return apply(native, this, arguments);
    } }[name], native);
    lock(arrayProto, name, guarded);
    if (name === "values") lock(arrayProto, iteratorSymbol, guarded);
  }

  // Array iterators read the length live on every step, so a getter can grow
  // the array mid-iteration; each iterator object is capped independently.
  const iterationCounts = new NativeWeakMap();
  lock(arrayIteratorPrototype, "next", withLength({ next() {
    if (isObject(this)) {
      const count = (apply(weakGet, iterationCounts, [this]) || 0) + 1;
      if (count > MAX_VALUE + 1) reject("array iteration");
      apply(weakSet, iterationCounts, [this, count]);
    }
    return apply(nativeArrayIteratorNext, this, []);
  } }.next, nativeArrayIteratorNext));

  const isConstructor = (value) => {
    if (typeof value !== "function") return false;
    try {
      construct(function () {}, [], value);
      return true;
    } catch (error) {
      return false;
    }
  };
  lock(NativeArray, "from", withLength({ from(items) {
    const mapper = arguments[1];
    const thisArg = arguments[2];
    if (mapper !== undefined && typeof mapper !== "function") {
      throw new NativeTypeError("Array.from mapper is not callable");
    }
    const values = [];
    let count = 0;
    const iteratorMethod = requireCoercible(items, "Array.from")[iteratorSymbol];
    if (iteratorMethod !== undefined && iteratorMethod !== null) {
      if (typeof iteratorMethod !== "function") {
        throw new NativeTypeError("Array.from iterator is not callable");
      }
      const iterator = apply(iteratorMethod, items, []);
      if (!isObject(iterator)) throw new NativeTypeError("Array.from iterator is not an object");
      const next = iterator.next;
      const close = () => {
        const finish = iterator.return;
        if (typeof finish === "function") apply(finish, iterator, []);
      };
      for (;;) {
        const step = apply(next, iterator, []);
        if (!isObject(step)) throw new NativeTypeError("iterator result is not an object");
        if (step.done) break;
        if (count >= MAX_VALUE) {
          try { close(); } catch (ignored) { /* the bound rejection wins */ }
          reject("Array.from length");
        }
        let value = step.value;
        if (mapper !== undefined) {
          try {
            value = apply(mapper, thisArg, [value, count]);
          } catch (error) {
            try { close(); } catch (ignored) { /* the mapper error wins */ }
            throw error;
          }
        }
        defineElement(values, count, value);
        count += 1;
      }
    } else {
      const object = NativeObject(items);
      const length = boundedLength(object, "Array.from length");
      for (; count < length; count += 1) {
        const value = object[count];
        defineElement(values, count,
          mapper === undefined ? value : apply(mapper, thisArg, [value, count]));
      }
    }
    if (this === NativeArray || !isConstructor(this)) return values;
    const result = construct(this, iteratorMethod === undefined || iteratorMethod === null
      ? [count] : []);
    for (let index = 0; index < count; index += 1) {
      defineProperty(result, index, {
        value: values[index], writable: true, enumerable: true, configurable: true,
      });
    }
    result.length = count;
    return result;
  } }.from, NativeArray.from));

  // ---- Argument lists ----------------------------------------------------

  const boundedArguments = (list) => {
    if (isObject(list)) boundedLength(list, "argument list length");
    return list;
  };
  // BOOTSTRAP removed the global `Function`; reach its prototype directly.
  const functionProto = getPrototypeOf(function () {});
  const nativeFunctionApply = functionProto.apply;
  lock(functionProto, "apply", withLength({ apply(thisArg, argArray) {
    if (argArray === undefined || argArray === null) return apply(this, thisArg, []);
    return apply(this, thisArg, boundedArguments(argArray));
  } }.apply, nativeFunctionApply));
  lock(Reflect, "apply", withLength({ apply(target, thisArg, argumentsList) {
    return apply(target, thisArg, boundedArguments(argumentsList));
  } }.apply, apply));
  lock(Reflect, "construct", withLength({ construct(target, argumentsList) {
    const newTarget = arguments.length < 3 ? target : arguments[2];
    return construct(target, boundedArguments(argumentsList), newTarget);
  } }.construct, construct));

  // ---- JSON --------------------------------------------------------------

  // Primitive value of a String/Number/Boolean wrapper, or `undefined`.
  const unwrap = (value) => {
    try { apply(nativeStringValueOf, value, []); return toText(value); } catch (e) { /* not a String */ }
    try { apply(nativeNumberValueOf, value, []); return +value; } catch (e) { /* not a Number */ }
    try { return apply(nativeBooleanValueOf, value, []); } catch (e) { /* not a Boolean */ }
    return undefined;
  };
  const jsonGap = (space) => {
    let value = space;
    if (isObject(value)) {
      const primitive = unwrap(value);
      if (typeof primitive === "string" || typeof primitive === "number") value = primitive;
    }
    if (typeof value === "number") {
      const width = min(10, toIntegerOrInfinity(value));
      return width >= 1 ? apply(nativeRepeat, " ", [width]) : "";
    }
    if (typeof value === "string") return apply(nativeSlice, value, [0, 10]);
    return "";
  };
  const jsonPropertyList = (replacer) => {
    const length = boundedLength(replacer, "JSON.stringify property list");
    const list = [];
    const seen = objectCreate(null);
    for (let index = 0; index < length; index += 1) {
      const element = replacer[index];
      let item;
      if (typeof element === "string") item = element;
      else if (typeof element === "number") item = toText(element);
      else if (isObject(element)) {
        const primitive = unwrap(element);
        if (typeof primitive === "string" || typeof primitive === "number") item = toText(element);
      }
      if (item !== undefined && !hasOwn(seen, item)) {
        seen[item] = true;
        append(list, item);
      }
    }
    return list;
  };
  lock(JSON, "stringify", withLength({ stringify(value, replacer, space) {
    const userReplacer = typeof replacer === "function" ? replacer : undefined;
    const propertyList = userReplacer === undefined && isArray(replacer)
      ? jsonPropertyList(replacer) : undefined;
    const gap = jsonGap(space);
    const depths = new NativeWeakMap();
    let total = 0;
    const track = function (key, current) {
      let result = userReplacer === undefined ? current : apply(userReplacer, this, [key, current]);
      const depth = (apply(weakGet, depths, [this]) || 0) + 1;
      total += (isArray(this) ? 1 : key.length + 4) + gap.length * depth + 1;
      if (isObject(result) && typeof result !== "function") {
        const primitive = unwrap(result);
        if (primitive !== undefined) result = primitive;
      }
      if (typeof result === "string") total += result.length + 2;
      else if (typeof result === "number" || typeof result === "boolean") total += toText(result).length;
      else if (result === null) total += 4;
      else if (typeof result === "object") {
        if (depth > MAX_NESTING) reject("JSON nesting");
        total += 2 + gap.length * depth;
        if (propertyList !== undefined && !isArray(result)) {
          const filtered = objectCreate(null);
          for (let index = 0; index < propertyList.length; index += 1) {
            const name = propertyList[index];
            defineProperty(filtered, name, {
              value: result[name], writable: true, enumerable: true, configurable: true,
            });
          }
          result = filtered;
        }
        apply(weakSet, depths, [result, depth]);
      }
      if (total > MAX_JSON) reject("JSON text");
      return result;
    };
    return apply(nativeStringify, JSON, [value, track, gap]);
  } }.stringify, nativeStringify));
  // Boa parses JSON text with its recursive ECMAScript parser, so array and
  // object nesting (outside string literals) is bounded before it runs.
  lock(JSON, "parse", withLength({ parse(text, reviver) {
    const source = toText(text);
    if (source.length > MAX_VALUE) reject("JSON.parse text");
    let depth = 0;
    let inString = false;
    for (let index = 0; index < source.length; index += 1) {
      const code = apply(nativeCharCodeAt, source, [index]);
      if (inString) {
        if (code === 92) index += 1;
        else if (code === 34) inString = false;
      } else if (code === 34) {
        inString = true;
      } else if (code === 91 || code === 123) {
        depth += 1;
        if (depth > MAX_NESTING) reject("JSON nesting");
      } else if (code === 93 || code === 125) {
        depth -= 1;
      }
    }
    return arguments.length < 2
      ? apply(nativeParse, JSON, [source])
      : apply(nativeParse, JSON, [source, reviver]);
  } }.parse, nativeParse));
})

// Vendored Babel-style asyncToGenerator, matching the runtime contract of
// @oxc-project/runtime/helpers/asyncToGenerator.
function step(gen, resolve, reject, next, raise, key, arg) {
  let info;
  try {
    info = gen[key](arg);
  } catch (error) {
    reject(error);
    return;
  }
  if (info.done) {
    resolve(info.value);
  } else {
    Promise.resolve(info.value).then(next, raise);
  }
}

export default function _asyncToGenerator(fn) {
  return function () {
    const self = this;
    const args = arguments;
    return new Promise((resolve, reject) => {
      const gen = fn.apply(self, args);
      function next(value) {
        step(gen, resolve, reject, next, raise, "next", value);
      }
      function raise(err) {
        step(gen, resolve, reject, next, raise, "throw", err);
      }
      next(undefined);
    });
  };
}

// A host entry that never returns from apply(), for the timeout test.
//
// It also refuses to exit on its own, so the test can prove the host kills the
// child rather than waiting for it to finish.
module.exports = {
  apply() {
    return new Promise(() => {});
  },
};

const helper = require("./does-not-exist");

test("never runs", () => {
  expect(helper).toBe(1);
});

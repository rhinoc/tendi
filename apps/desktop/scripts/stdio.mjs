export function writeStdout(...values) {
  process.stdout.write(`${values.join(" ")}\n`);
}

export function writeStderr(...values) {
  process.stderr.write(`${values.join(" ")}\n`);
}

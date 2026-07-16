// Instantiates the -sMODULARIZE build twice in one process with interleaved
// suspensions; both instances must report marker:instance-ok.
const path = require('path');

const factory = require(path.resolve(process.argv[2]));

function run(tag) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`${tag}: timeout`)), 10000);
    factory({
      print: (s) => {
        console.log(tag, s);
        if (s.includes('marker:instance-ok')) {
          clearTimeout(timer);
          resolve();
        }
      },
      printErr: (s) => console.error(tag, s),
    }).catch(reject);
  });
}

Promise.all([run('A'), run('B')]).then(
  () => {
    console.log('two-instance ok');
    process.exit(0);
  },
  (e) => {
    console.error(e);
    process.exit(1);
  },
);

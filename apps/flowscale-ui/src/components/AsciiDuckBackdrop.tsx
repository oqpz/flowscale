const ASCII_DUCK_ART = [
  '             ::::::::::::::::',
  '        :::::::::::::::::::::::::::::',
  '     ::::::::::::::::::::::::::::::::::',
  '      :::::::::############:::::::::::',
  '   :::::::::#########++++++++:::::::::::',
  '   :::::::#####++++++++++++::::::::::::',
  '   :::::::###++++++++:::::::::::::::::::::',
  '     :::::::::::::::::::::::::::::::::::::',
  '   :::::::::::::       :::::::::::::::::::',
  '   :::::::::::           :::::::::::::::::',
  '   :::::::                 :::::::::::::::',
  '                           :::::::::::::::',
  '                           :::::::::::::::',
  '                           :::::::::::::::::',
  '                         :::::::::::::::::::::',
  '                         ::::@@..::::==========',
  '                         ::::@@@@::::========',
  '                           ::::::::::======',
  '                             :::::::::::',
  '                               :::::::',
].join('\n');

export function AsciiDuckBackdrop() {
  return (
    <div className="fs-ascii-backdrop" aria-hidden="true" role="presentation">
      <pre className="fs-ascii-duck" aria-hidden="true">
        {ASCII_DUCK_ART}
      </pre>
    </div>
  );
}

export function HonkCornerSignature() {
  return (
    <div className="fs-honk-corner" aria-hidden="true" role="presentation">
      <pre aria-hidden="true">{`  _(o)>\n\\<_.)\n  \`--'`}</pre>
      <span className="fs-honk-label">Honk!</span>
    </div>
  );
}

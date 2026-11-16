const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

function loadPlayers() {
    const window = { api: { player: { setVolume() {} } } };
    const context = vm.createContext({ window, console: { debug() {}, error() {} } });
    for (const file of ['mpv-player-base.js', 'mpv-video-player.js']) {
        vm.runInContext(fs.readFileSync(path.join(__dirname, '..', file), 'utf8'), context);
    }
    return window;
}

test('playback statistics preserve zeroes, omit unavailable values, and escape HTML', async () => {
    const window = loadPlayers();
    window._playbackDiagnostics = {
        videoCodec: '<img src=x onerror=alert(1)>',
        hardwareDecoding: 'no',
        droppedFrames: 0,
        decoderDroppedFrames: null,
        cacheDuration: 0,
        cacheBytes: 4096
    };
    const player = Object.create(window._mpvVideoPlayer.prototype);
    player._currentPlayOptions = {
        playMethod: 'DirectPlay',
        url: 'https://user:password@server/video?api_key=secret',
        item: { Name: 'Private title' }
    };
    const stats = await player.getStats();
    const rows = stats.categories.flatMap(category => category.stats);
    assert.equal(rows.find(row => row.label === 'Dropped frames (output)').value, '0');
    assert.equal(rows.find(row => row.label === 'Buffered time (approximate)').value, '00m 00s');
    assert.equal(rows.find(row => row.label === 'Play method').value, 'DirectPlay');
    assert.equal(rows.find(row => row.label === 'Codec').value, '&lt;img src=x onerror=alert(1)&gt;');
    assert.equal(rows.some(row => row.label === 'Dropped frames (decoder)'), false);
    const report = JSON.stringify(stats);
    for (const privateValue of ['password', 'api_key', 'secret', 'Private title']) {
        assert.equal(report.includes(privateValue), false);
    }
});

test('buffer units and input rate formatting preserve counters and native data', async () => {
    const window = loadPlayers();
    const diagnostics = Object.freeze({
        cacheDuration: 12.3456789,
        cacheBytes: 123456789,
        inputBytesPerSecond: 987654.321,
        droppedFrames: 1234,
        decoderDroppedFrames: 5678
    });
    window._playbackDiagnostics = diagnostics;
    const player = Object.create(window._mpvVideoPlayer.prototype);
    const stats = await player.getStats();
    const rows = stats.categories.flatMap(category => category.stats);
    assert.equal(rows.find(row => row.label === 'Buffered time (approximate)').value, '00m 12s');
    assert.equal(rows.find(row => row.label === 'Buffered data').value, `${(123).toLocaleString()} MB`);
    assert.equal(rows.find(row => row.label === 'Input rate').value, `${(0.988).toLocaleString()} MB/s`);
    assert.equal(rows.find(row => row.label === 'Dropped frames (output)').value, '1234');
    assert.equal(rows.find(row => row.label === 'Dropped frames (decoder)').value, '5678');
    assert.equal(diagnostics.cacheDuration, 12.3456789);
    assert.equal(diagnostics.cacheBytes, 123456789);
    assert.equal(diagnostics.inputBytesPerSecond, 987654.321);
});

test('subsecond buffers use whole seconds and unavailable measurements stay omitted', async () => {
    const window = loadPlayers();
    window._playbackDiagnostics = {
        cacheDuration: 0.0123456789,
        cacheBytes: null,
        inputBytesPerSecond: Infinity
    };
    const player = Object.create(window._mpvVideoPlayer.prototype);
    const stats = await player.getStats();
    const rows = stats.categories.flatMap(category => category.stats);
    assert.equal(rows.find(row => row.label === 'Buffered time (approximate)').value, '00m 00s');
    assert.equal(rows.some(row => row.label === 'Buffered data'), false);
    assert.equal(rows.some(row => row.label === 'Input rate'), false);
});

test('buffered durations pad minutes and seconds and carry rounded seconds correctly', async () => {
    const window = loadPlayers();
    const player = Object.create(window._mpvVideoPlayer.prototype);
    for (const [cacheDuration, expected] of [
        [0, '00m 00s'],
        [5.4, '00m 05s'],
        [65.4, '01m 05s'],
        [59.6, '01m 00s'],
        [3599.6, '60m 00s'],
        [6000, '100m 00s']
    ]) {
        window._playbackDiagnostics = { cacheDuration };
        const stats = await player.getStats();
        const rows = stats.categories.flatMap(category => category.stats);
        assert.equal(rows.find(row => row.label === 'Buffered time (approximate)').value, expected);
    }
    for (const cacheDuration of [null, undefined, NaN, Infinity]) {
        window._playbackDiagnostics = { cacheDuration };
        const stats = await player.getStats();
        assert.equal(stats.categories.length, 0);
    }
});

test('buffered data uses decimal megabytes with up to three significant figures', async () => {
    const window = loadPlayers();
    const player = Object.create(window._mpvVideoPlayer.prototype);
    for (const [cacheBytes, expected] of [[0, 0], [1_000_000, 1], [1_500_000, 1.5], [1_048_576, 1.05]]) {
        window._playbackDiagnostics = { cacheBytes };
        const stats = await player.getStats();
        const rows = stats.categories.flatMap(category => category.stats);
        assert.equal(rows.find(row => row.label === 'Buffered data').value, `${expected.toLocaleString()} MB`);
    }
});

test('input rate uses decimal MB/s and preserves zero and small measurements', async () => {
    const window = loadPlayers();
    const player = Object.create(window._mpvVideoPlayer.prototype);
    for (const [inputBytesPerSecond, expected] of [[0, 0], [1_000_000, 1], [1_500_000, 1.5], [1_048_576, 1.05], [123, 0.000123]]) {
        window._playbackDiagnostics = { inputBytesPerSecond };
        const stats = await player.getStats();
        const rows = stats.categories.flatMap(category => category.stats);
        assert.equal(rows.find(row => row.label === 'Input rate').value, `${expected.toLocaleString(undefined, { maximumFractionDigits: 6 })} MB/s`);
    }
    for (const inputBytesPerSecond of [null, undefined, NaN, Infinity]) {
        window._playbackDiagnostics = { inputBytesPerSecond };
        const stats = await player.getStats();
        assert.equal(stats.categories.length, 0);
    }
});

test('audio and video preserve native error categories, codes, and recovery details', () => {
    const window = loadPlayers();
    const received = [];
    const args = {
        events: { trigger(_player, name, values) { received.push({ name, error: values[0] }); } },
        appSettings: { get() { return 1; }, set() {} },
        appHost: {}
    };
    const error = { type: 'NETWORK_ERROR', code: -13, message: 'loading failed', guidance: 'Check connectivity' };
    const audio = new window.MpvPlayerBase(args);
    const video = new window._mpvVideoPlayer(args);
    let removed = false;
    video.removeMediaDialog = () => { removed = true; };
    audio.handlers.onError(error);
    video.handlers.onError(error);
    assert.equal(received.length, 2);
    assert.equal(received[0].error, error);
    assert.equal(received[1].error, error);
    assert.equal(removed, true);
    assert.equal(received.every(event => event.name === 'error'), true);
});

test('statistics are empty before native observations arrive', async () => {
    const window = loadPlayers();
    const player = Object.create(window._mpvVideoPlayer.prototype);
    const stats = await player.getStats();
    assert.equal(stats.categories.length, 0);
});

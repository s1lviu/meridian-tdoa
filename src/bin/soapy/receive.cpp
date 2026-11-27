// Receive-only bridge for the SoapySDR stream flags and the GPSDO sensors.
#include <SoapySDR/Device.hpp>
#include <SoapySDR/Formats.hpp>
#include <SoapySDR/Errors.hpp>
#include <algorithm>
#include <chrono>
#include <cmath>
#include <complex>
#include <cstdint>
#include <cstdio>
#include <fstream>
#include <memory>
#include <stdexcept>
#include <thread>
#include <vector>

using Device = SoapySDR::Device;
using namespace std::chrono_literals;
constexpr int64_t second = 1000000000;

static void check(bool ok, const char *message) {
    if (!ok) throw std::runtime_error(message);
}

static void check_lock(Device &d) {
    check(d.readSensor<bool>("gps_locked") && d.readSensor<bool>("ref_locked"),
          "GPS or sample clock is unlocked");
}

// Pair the last PPS tick with the GPSDO's Unix UTC second. UHD's GPS sentence
// follows the edge. Sample after the documented settling time and bracket the
// sensor read to rule out a rollover. Never initialize UTC from the host clock.
static int64_t utc_offset(Device &d) {
    check_lock(d);
    const auto previous = d.getHardwareTime("PPS");
    const auto deadline = std::chrono::steady_clock::now() + 2s;
    while (d.getHardwareTime("PPS") == previous) {
        check(std::chrono::steady_clock::now() < deadline, "Missing PPS edge");
        std::this_thread::sleep_for(5ms);
    }
    std::this_thread::sleep_for(250ms);
    const auto edge = d.getHardwareTime("PPS");
    const auto utc = std::stoll(d.readSensor("gps_time"));
    check(utc > 0 && utc < INT64_MAX / second && edge >= 0 &&
          edge == d.getHardwareTime("PPS"), "Invalid GPS UTC/PPS pair");
    return utc * second - edge;
}

struct Measurement {
    int64_t utc_ns;
    double sample_rate, frequency;
    uint64_t samples;
};

extern "C" int meridian_receive(const char *args, size_t channel, double rate,
        double frequency, double gain, double duration, int64_t start_ns,
        const char *path, Measurement *out, char *error, size_t error_size) {
    try {
        auto release = [](Device *p) { Device::unmake(p); };
        std::unique_ptr<Device, decltype(release)> d(Device::make(args), release);
        check(bool(d), "No matching SDR");
        d->setClockSource("gpsdo");
        d->setTimeSource("gpsdo");
        check(d->getClockSource() == "gpsdo" && d->getTimeSource() == "gpsdo" &&
              d->hasHardwareTime("PPS"), "The driver must expose a GPSDO and last-PPS hardware time");
        d->setSampleRate(SOAPY_SDR_RX, channel, rate);
        d->setFrequency(SOAPY_SDR_RX, channel, frequency);
        if (std::isfinite(gain)) d->setGain(SOAPY_SDR_RX, channel, gain);
        out->sample_rate = d->getSampleRate(SOAPY_SDR_RX, channel);
        out->frequency = d->getFrequency(SOAPY_SDR_RX, channel);
        const auto count = std::ceil(duration * out->sample_rate);
        check(out->sample_rate >= 12000 && count >= 1 && count <= 512 * 1024 * 1024 / 8,
              "Capture must fit the SigMF reader's 512 MiB limit");
        out->samples = static_cast<uint64_t>(count);
        const int64_t offset = utc_offset(*d);

        auto close = [&](SoapySDR::Stream *s) { d->closeStream(s); };
        std::unique_ptr<SoapySDR::Stream, decltype(close)> stream(
            d->setupStream(SOAPY_SDR_RX, SOAPY_SDR_CF32, {channel}), close);
        check(bool(stream), "Could not open RX stream");
        const size_t mtu = d->getStreamMTU(stream.get());
        check(mtu > 0, "Driver returned an empty stream MTU");
        std::vector<std::complex<float>> buffer(std::min<size_t>(mtu, 65536));
        std::ofstream file(path, std::ios::binary);
        file.exceptions(std::ios::badbit | std::ios::failbit);
        if (start_ns) {
            const auto start_tick = start_ns - offset;
            // Wait before activation so the first read can use its normal timeout.
            while (start_tick - d->getHardwareTime() > second) std::this_thread::sleep_for(50ms);
            check(start_tick > d->getHardwareTime(), "Scheduled UTC start has passed");
        }
        check(d->activateStream(stream.get(), start_ns ? SOAPY_SDR_HAS_TIME : 0,
              start_ns ? start_ns - offset : 0) == 0, "Could not activate RX stream");
        auto next_lock_check = std::chrono::steady_clock::now();
        int64_t first = 0;
        for (uint64_t done = 0; done < out->samples;) {
            if (std::chrono::steady_clock::now() >= next_lock_check) {
                check_lock(*d);
                next_lock_check = std::chrono::steady_clock::now() + 1s;
            }
            void *buffers[] = {buffer.data()};
            int flags = 0;
            long long tick = 0;
            const size_t want = std::min<uint64_t>(buffer.size(), out->samples - done);
            const int n = d->readStream(stream.get(), buffers, want, flags, tick, 2000000);
            if (n < 0) throw std::runtime_error(SoapySDR::errToStr(n));
            check(n > 0 && size_t(n) <= want && (flags & SOAPY_SDR_HAS_TIME) &&
                  !(flags & SOAPY_SDR_END_ABRUPT), "Missing RX timestamp or discontinuous stream");
            if (!done) {
                first = tick;
                check(!start_ns || std::abs(first - (start_ns - offset)) <= 1 + 0.5 * second / out->sample_rate,
                      "RX stream missed the requested UTC start");
            }
            const int64_t elapsed = std::llround(done * double(second) / out->sample_rate);
            check(std::abs(tick - first - elapsed) <= 1 + 0.5 * second / out->sample_rate,
                  "RX hardware timestamps disagree with the sample count");
            file.write(reinterpret_cast<const char *>(buffer.data()), n * sizeof(buffer[0]));
            done += size_t(n);
        }
        check(d->deactivateStream(stream.get()) == 0, "Could not stop RX stream");
        file.close();
        check(std::abs(utc_offset(*d) - offset) <= 1, "GPS UTC/PPS alignment changed during capture");
        out->utc_ns = first + offset;
        return 0;
    } catch (const std::exception &e) {
        std::snprintf(error, error_size, "%s", e.what());
        return -1;
    }
}

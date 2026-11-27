// Receive-only test device loaded by the real SoapySDR module registry.
#include <SoapySDR/Device.hpp>
#include <SoapySDR/Registry.hpp>
#include <SoapySDR/Formats.hpp>
#include <algorithm>
#include <chrono>
#include <complex>
#include <cmath>

constexpr long long second = 1000000000;
constexpr long long epoch = 1700000000;
class Fixture final : public SoapySDR::Device {
    std::chrono::steady_clock::time_point boot = std::chrono::steady_clock::now();
    std::string fault, clock, time;
    double rate = 0, frequency = 0;
    long long first = 0;
    size_t samples = 0;
public:
    explicit Fixture(const SoapySDR::Kwargs &args) : fault(args.count("fault") ? args.at("fault") : "") {}
    void setClockSource(const std::string &v) override { clock = v; }
    void setTimeSource(const std::string &v) override { time = v; }
    std::string getClockSource() const override { return clock; }
    std::string getTimeSource() const override { return time; }
    void setSampleRate(int, size_t, double r) override { rate = r; }
    double getSampleRate(int, size_t) const override { return rate; }
    void setFrequency(int, size_t, double f, const SoapySDR::Kwargs &) override { frequency=f; }
    double getFrequency(int, size_t) const override { return frequency; }
    bool hasHardwareTime(const std::string &) const override { return true; }
    long long getHardwareTime(const std::string &what) const override {
        auto ns = std::chrono::duration_cast<std::chrono::nanoseconds>(std::chrono::steady_clock::now()-boot).count();
        return what == "PPS" ? ns/second*second : ns;
    }
    std::string readSensor(const std::string &name) const override {
        if (name == "gps_time") return std::to_string(epoch + getHardwareTime("PPS")/second);
        return fault == "unlocked" ? "false" : "true";
    }
    SoapySDR::Stream *setupStream(int, const std::string &, const std::vector<size_t> &, const SoapySDR::Kwargs &) override {
        return reinterpret_cast<SoapySDR::Stream *>(this);
    }
    void closeStream(SoapySDR::Stream *) override {}
    size_t getStreamMTU(SoapySDR::Stream *) const override { return 256; }
    int activateStream(SoapySDR::Stream *, int flags, long long start, size_t) override {
        first = flags & SOAPY_SDR_HAS_TIME ? start : getHardwareTime("");
        return 0;
    }
    int deactivateStream(SoapySDR::Stream *, int, long long) override { return 0; }
    int readStream(SoapySDR::Stream *, void * const *buffers, size_t count, int &flags, long long &ns, long) override {
        flags = fault == "untimed" ? 0 : SOAPY_SDR_HAS_TIME;
        ns = first + std::llround(samples*double(second)/rate);
        if (fault == "gap" && samples) ns += second;
        std::fill_n(static_cast<std::complex<float> *>(buffers[0]), count, std::complex<float>(0.25,-0.5));
        samples += count;
        return int(count);
    }
};
static SoapySDR::KwargsList findFixture(const SoapySDR::Kwargs &args) {
    if (!args.count("driver") || args.at("driver") != "meridian_fixture") return {};
    return {args};
}
static SoapySDR::Device *makeFixture(const SoapySDR::Kwargs &args) { return new Fixture(args); }
static SoapySDR::Registry reg("meridian_fixture", findFixture, makeFixture, SOAPY_SDR_ABI_VERSION);

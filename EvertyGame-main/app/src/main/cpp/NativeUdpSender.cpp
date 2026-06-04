#include <jni.h>

#include <algorithm>
#include <arpa/inet.h>
#include <atomic>
#include <cerrno>
#include <cstdint>
#include <cstring>
#include <memory>
#include <mutex>
#include <netdb.h>
#include <string>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>
#include <vector>

namespace {

constexpr uint32_t kMagic = 0x45565254;
constexpr uint8_t kVersion = 3;
constexpr uint8_t kTypeVideoFrame = 3;
constexpr uint8_t kTypeControl = 4;
constexpr size_t kHeaderSize = 24;
constexpr size_t kMaxPacketSize = 1200;
constexpr size_t kMaxPayloadSize = kMaxPacketSize - kHeaderSize;
constexpr uint16_t kFlagKeyFrame = 1;

int64_t MonotonicNowNs() {
    timespec ts{};
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return static_cast<int64_t>(ts.tv_sec) * 1000000000LL + static_cast<int64_t>(ts.tv_nsec);
}

void SleepUntilMonotonicNs(int64_t deadline_ns) {
    if (deadline_ns <= 0) {
        return;
    }

    timespec ts{};
    ts.tv_sec = static_cast<time_t>(deadline_ns / 1000000000LL);
    ts.tv_nsec = static_cast<long>(deadline_ns % 1000000000LL);
    while (clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &ts, nullptr) == EINTR) {
    }
}

int64_t ComputePacingWindowNs(int payload_size, uint16_t packet_count) {
    if (packet_count <= 3 || payload_size <= static_cast<int>(kMaxPayloadSize * 3)) {
        return 0;
    }

    auto total_window_ns = static_cast<int64_t>(packet_count - 1) * 120000LL;
    if (payload_size >= static_cast<int>(kMaxPayloadSize * 12)) {
        total_window_ns = std::max<int64_t>(total_window_ns, 1500000LL);
    }

    return std::clamp<int64_t>(total_window_ns, 500000LL, 3000000LL);
}

struct NativeUdpContext {
    std::atomic<int> socket_fd{-1};
    std::mutex io_mutex;
};

void ThrowIOException(JNIEnv* env, const std::string& message) {
    jclass exception_class = env->FindClass("java/io/IOException");
    if (exception_class == nullptr) {
        env->FatalError(message.c_str());
    }
    env->ThrowNew(exception_class, message.c_str());
}

NativeUdpContext* GetContext(jlong handle) {
    return reinterpret_cast<NativeUdpContext*>(handle);
}

void WriteInt32BE(uint8_t* dst, uint32_t value) {
    dst[0] = static_cast<uint8_t>((value >> 24) & 0xFF);
    dst[1] = static_cast<uint8_t>((value >> 16) & 0xFF);
    dst[2] = static_cast<uint8_t>((value >> 8) & 0xFF);
    dst[3] = static_cast<uint8_t>(value & 0xFF);
}

void WriteInt16BE(uint8_t* dst, uint16_t value) {
    dst[0] = static_cast<uint8_t>((value >> 8) & 0xFF);
    dst[1] = static_cast<uint8_t>(value & 0xFF);
}

void WriteInt64BE(uint8_t* dst, uint64_t value) {
    for (int i = 7; i >= 0; --i) {
        dst[7 - i] = static_cast<uint8_t>((value >> (i * 8)) & 0xFF);
    }
}

void BuildPacketHeader(
    uint8_t* dst,
    uint8_t type,
    uint16_t flags,
    uint32_t frame_id,
    uint16_t packet_index,
    uint16_t packet_count,
    uint64_t presentation_time_us) {
    WriteInt32BE(dst, kMagic);
    dst[4] = kVersion;
    dst[5] = type;
    WriteInt16BE(dst + 6, flags);
    WriteInt32BE(dst + 8, frame_id);
    WriteInt16BE(dst + 12, packet_index);
    WriteInt16BE(dst + 14, packet_count);
    WriteInt64BE(dst + 16, presentation_time_us);
}

void ConfigureSocket(int socket_fd, int family) {
    const int one_mb = 1 << 20;
    setsockopt(socket_fd, SOL_SOCKET, SO_SNDBUF, &one_mb, sizeof(one_mb));
    setsockopt(socket_fd, SOL_SOCKET, SO_RCVBUF, &one_mb, sizeof(one_mb));

    const int low_delay = 0x10;
    if (family == AF_INET) {
        setsockopt(socket_fd, IPPROTO_IP, IP_TOS, &low_delay, sizeof(low_delay));
    }
#ifdef IPV6_TCLASS
    if (family == AF_INET6) {
        setsockopt(socket_fd, IPPROTO_IPV6, IPV6_TCLASS, &low_delay, sizeof(low_delay));
    }
#endif
}

}  // namespace

extern "C" JNIEXPORT jlong JNICALL
Java_com_everty_evertygame_stream_NativeUdpSender_nativeCreate(
    JNIEnv* env,
    jobject /* thiz */,
    jstring host,
    jint port) {
    const char* host_chars = env->GetStringUTFChars(host, nullptr);
    if (host_chars == nullptr) {
        ThrowIOException(env, "Failed to read UDP host");
        return 0;
    }

    std::string host_string(host_chars);
    env->ReleaseStringUTFChars(host, host_chars);

    addrinfo hints{};
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_DGRAM;
    hints.ai_protocol = IPPROTO_UDP;

    addrinfo* result = nullptr;
    const std::string port_string = std::to_string(port);
    const int resolve_rc = getaddrinfo(host_string.c_str(), port_string.c_str(), &hints, &result);
    if (resolve_rc != 0 || result == nullptr) {
        ThrowIOException(env, "Failed to resolve UDP host: " + host_string);
        return 0;
    }

    std::unique_ptr<NativeUdpContext> context(new NativeUdpContext());
    for (addrinfo* candidate = result; candidate != nullptr; candidate = candidate->ai_next) {
        const int fd = socket(candidate->ai_family, candidate->ai_socktype, candidate->ai_protocol);
        if (fd < 0) {
            continue;
        }

        ConfigureSocket(fd, candidate->ai_family);
        if (connect(fd, candidate->ai_addr, candidate->ai_addrlen) == 0) {
            context->socket_fd.store(fd);
            break;
        }

        close(fd);
    }
    freeaddrinfo(result);

    if (context->socket_fd.load() < 0) {
        ThrowIOException(env, "Failed to connect UDP socket to " + host_string + ":" + port_string);
        return 0;
    }

    return reinterpret_cast<jlong>(context.release());
}

extern "C" JNIEXPORT void JNICALL
Java_com_everty_evertygame_stream_NativeUdpSender_nativeSendPacket(
    JNIEnv* env,
    jobject /* thiz */,
    jlong native_handle,
    jbyteArray packet,
    jint packet_size) {
    NativeUdpContext* context = GetContext(native_handle);
    if (context == nullptr || context->socket_fd.load() < 0) {
        ThrowIOException(env, "Native UDP sender is not initialized");
        return;
    }

    std::vector<uint8_t> bytes(packet_size);
    env->GetByteArrayRegion(packet, 0, packet_size, reinterpret_cast<jbyte*>(bytes.data()));

    std::lock_guard<std::mutex> lock(context->io_mutex);
    const int fd = context->socket_fd.load();
    if (fd < 0) {
        ThrowIOException(env, "Native UDP sender socket is closed");
        return;
    }
    const ssize_t sent = send(fd, bytes.data(), bytes.size(), 0);
    if (sent < 0) {
        ThrowIOException(env, "Native UDP send failed: " + std::string(strerror(errno)));
        return;
    }
}

extern "C" JNIEXPORT jint JNICALL
Java_com_everty_evertygame_stream_NativeUdpSender_nativeSendVideoFrame(
    JNIEnv* env,
    jobject /* thiz */,
    jlong native_handle,
    jint frame_id,
    jlong presentation_time_us,
    jboolean is_key_frame,
    jobject payload_buffer,
    jint payload_offset,
    jint payload_size) {
    NativeUdpContext* context = GetContext(native_handle);
    if (context == nullptr || context->socket_fd.load() < 0) {
        ThrowIOException(env, "Native UDP sender is not initialized");
        return 0;
    }

    auto* payload_base = static_cast<uint8_t*>(env->GetDirectBufferAddress(payload_buffer));
    if (payload_base == nullptr) {
        ThrowIOException(env, "MediaCodec output buffer is not direct");
        return 0;
    }

    if (payload_offset < 0 || payload_size <= 0) {
        ThrowIOException(env, "Invalid encoded payload range");
        return 0;
    }

    const uint8_t* payload = payload_base + payload_offset;
    const uint16_t packet_count =
        static_cast<uint16_t>((payload_size + static_cast<jint>(kMaxPayloadSize) - 1) / static_cast<jint>(kMaxPayloadSize));
    const uint16_t flags = is_key_frame ? kFlagKeyFrame : 0;
    const int64_t pacing_window_ns = ComputePacingWindowNs(payload_size, packet_count);
    const int64_t pacing_step_ns =
        packet_count > 1 && pacing_window_ns > 0
            ? pacing_window_ns / static_cast<int64_t>(packet_count - 1)
            : 0;
    const int64_t pacing_start_ns = pacing_step_ns > 0 ? MonotonicNowNs() : 0;

    std::vector<uint8_t> packet(kMaxPacketSize);
    std::lock_guard<std::mutex> lock(context->io_mutex);
    const int fd = context->socket_fd.load();
    if (fd < 0) {
        ThrowIOException(env, "Native UDP sender socket is closed");
        return 0;
    }
    for (uint16_t packet_index = 0; packet_index < packet_count; ++packet_index) {
        const size_t chunk_offset = static_cast<size_t>(packet_index) * kMaxPayloadSize;
        const size_t chunk_size =
            std::min(kMaxPayloadSize, static_cast<size_t>(payload_size) - chunk_offset);

        BuildPacketHeader(
            packet.data(),
            kTypeVideoFrame,
            flags,
            static_cast<uint32_t>(frame_id),
            packet_index,
            packet_count,
            static_cast<uint64_t>(presentation_time_us));
        std::memcpy(packet.data() + kHeaderSize, payload + chunk_offset, chunk_size);

        const size_t packet_size = kHeaderSize + chunk_size;
        const ssize_t sent = send(fd, packet.data(), packet_size, 0);
        if (sent < 0) {
            ThrowIOException(env, "Native UDP video send failed: " + std::string(strerror(errno)));
            return 0;
        }

        if (pacing_step_ns > 0 && packet_index + 1 < packet_count) {
            const int64_t deadline_ns =
                pacing_start_ns + static_cast<int64_t>(packet_index + 1) * pacing_step_ns;
            SleepUntilMonotonicNs(deadline_ns);
        }
    }

    return packet_count;
}

extern "C" JNIEXPORT jbyteArray JNICALL
Java_com_everty_evertygame_stream_NativeUdpSender_nativeReceiveControlPayload(
    JNIEnv* env,
    jobject /* thiz */,
    jlong native_handle) {
    NativeUdpContext* context = GetContext(native_handle);
    if (context == nullptr) {
        return nullptr;
    }

    const int fd = context->socket_fd.load();
    if (fd < 0) {
        return nullptr;
    }

    std::vector<uint8_t> packet(kMaxPacketSize);
    const ssize_t received = recv(fd, packet.data(), packet.size(), 0);
    if (received <= 0) {
        if (errno == EBADF || errno == ECONNRESET || errno == ENOTCONN ||
            errno == ECONNREFUSED || errno == EHOSTUNREACH || errno == EAGAIN || errno == EWOULDBLOCK) {
            return nullptr;
        }
        ThrowIOException(env, "Native UDP receive failed: " + std::string(strerror(errno)));
        return nullptr;
    }

    if (received < static_cast<ssize_t>(kHeaderSize) || packet[5] != kTypeControl) {
        return nullptr;
    }

    const jsize payload_size = static_cast<jsize>(received - static_cast<ssize_t>(kHeaderSize));
    if (payload_size <= 0) {
        return nullptr;
    }

    jbyteArray payload = env->NewByteArray(payload_size);
    if (payload == nullptr) {
        ThrowIOException(env, "Failed to allocate control payload array");
    }
    env->SetByteArrayRegion(
        payload,
        0,
        payload_size,
        reinterpret_cast<const jbyte*>(packet.data() + kHeaderSize));
    return payload;
}

extern "C" JNIEXPORT void JNICALL
Java_com_everty_evertygame_stream_NativeUdpSender_nativeShutdown(
    JNIEnv* /* env */,
    jobject /* thiz */,
    jlong native_handle) {
    NativeUdpContext* context = GetContext(native_handle);
    if (context == nullptr) {
        return;
    }

    {
        std::lock_guard<std::mutex> lock(context->io_mutex);
        const int fd = context->socket_fd.exchange(-1);
        if (fd >= 0) {
            shutdown(fd, SHUT_RDWR);
            close(fd);
        }
    }
}

extern "C" JNIEXPORT void JNICALL
Java_com_everty_evertygame_stream_NativeUdpSender_nativeDestroy(
    JNIEnv* /* env */,
    jobject /* thiz */,
    jlong native_handle) {
    NativeUdpContext* context = GetContext(native_handle);
    if (context == nullptr) {
        return;
    }
    delete context;
}

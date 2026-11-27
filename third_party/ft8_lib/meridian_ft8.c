// SPDX-License-Identifier: MIT
// Meridian wrapper around ft8_lib: decode one 15 s FT8 slot of real 12 kHz audio and
// return payloads, unpacked fields and the exact channel tones of every decode.
// Not thread-safe (ft8_lib's callsign hash table is process-global); callers serialize.
#include <string.h>
#include <stdlib.h>
#include <ft8/decode.h>
#include <ft8/encode.h>
#include <ft8/message.h>
#include <common/monitor.h>

typedef struct
{
    float freq_hz;  // audio frequency of tone 0
    float time_s;   // start of the first symbol, seconds from the buffer start
    int score;      // Costas sync score
    int ldpc_errors;
    unsigned char payload[FTX_PAYLOAD_LENGTH_BYTES];
    unsigned char tones[FT8_NN];
    char text[FTX_MAX_MESSAGE_LENGTH + 1];
    char call_to[16];
    char call_de[16];
    char extra[16];
    int field_types[FTX_MAX_MESSAGE_FIELDS];
    int rc; // ftx_message_rc_t of the unpack
} mft8_decode_t;

#define HASH_SIZE 1024
static struct
{
    char callsign[12];
    uint32_t hash;
} table[HASH_SIZE];

static void hash_add(const char* callsign, uint32_t hash)
{
    uint16_t hash10 = (hash >> 12) & 0x3FFu;
    int idx = (hash10 * 23) % HASH_SIZE;
    for (int n = 0; n < HASH_SIZE && table[idx].callsign[0] != '\0'; ++n)
    {
        if ((table[idx].hash & 0x3FFFFFu) == hash && 0 == strcmp(table[idx].callsign, callsign))
            return;
        idx = (idx + 1) % HASH_SIZE;
    }
    if (table[idx].callsign[0] != '\0')
        return; // table full: the call stays unresolved (<...>), never guessed
    strncpy(table[idx].callsign, callsign, 11);
    table[idx].callsign[11] = '\0';
    table[idx].hash = hash;
}

static bool hash_lookup(ftx_callsign_hash_type_t type, uint32_t hash, char* callsign)
{
    uint8_t shift = (type == FTX_CALLSIGN_HASH_10_BITS) ? 12 : (type == FTX_CALLSIGN_HASH_12_BITS ? 10 : 0);
    uint16_t hash10 = (hash >> (12 - shift)) & 0x3FFu;
    int idx = (hash10 * 23) % HASH_SIZE;
    for (int n = 0; n < HASH_SIZE && table[idx].callsign[0] != '\0'; ++n)
    {
        if (((table[idx].hash & 0x3FFFFFu) >> shift) == hash)
        {
            strcpy(callsign, table[idx].callsign);
            return true;
        }
        idx = (idx + 1) % HASH_SIZE;
    }
    callsign[0] = '\0';
    return false;
}

static ftx_callsign_hash_interface_t hash_if = { .lookup_hash = hash_lookup, .save_hash = hash_add };

void mft8_reset_hashes(void)
{
    memset(table, 0, sizeof(table));
}

void mft8_tones(const unsigned char* payload, unsigned char* tones)
{
    ft8_encode(payload, tones);
}

int mft8_decode(const float* signal, int num_samples, int sample_rate, mft8_decode_t* out, int max_out)
{
    const int max_candidates = 200;
    const int min_score = 10;
    const int ldpc_iterations = 25;
    monitor_config_t cfg = {
        .f_min = 100, .f_max = 3100, .sample_rate = sample_rate, .time_osr = 2, .freq_osr = 2, .protocol = FTX_PROTOCOL_FT8
    };
    monitor_t mon;
    monitor_init(&mon, &cfg);
    for (int pos = 0; pos + mon.block_size <= num_samples; pos += mon.block_size)
        monitor_process(&mon, signal + pos);
    ftx_candidate_t* cands = (ftx_candidate_t*)malloc(sizeof(ftx_candidate_t) * max_candidates);
    int n_cand = ftx_find_candidates(&mon.wf, max_candidates, cands, min_score);
    int n_out = 0;
    for (int i = 0; i < n_cand && n_out < max_out; ++i)
    {
        const ftx_candidate_t* c = &cands[i];
        ftx_message_t msg;
        ftx_decode_status_t status;
        if (!ftx_decode_candidate(&mon.wf, c, ldpc_iterations, &msg, &status))
            continue;
        bool duplicate = false;
        for (int k = 0; k < n_out; ++k)
            if (0 == memcmp(out[k].payload, msg.payload, FTX_PAYLOAD_LENGTH_BYTES))
                duplicate = true;
        if (duplicate)
            continue;
        mft8_decode_t* d = &out[n_out++];
        memset(d, 0, sizeof(*d));
        d->freq_hz = (mon.min_bin + c->freq_offset + (float)c->freq_sub / mon.wf.freq_osr) / mon.symbol_period;
        d->time_s = (c->time_offset + (float)c->time_sub / mon.wf.time_osr) * mon.symbol_period;
        d->score = c->score;
        d->ldpc_errors = status.ldpc_errors;
        memcpy(d->payload, msg.payload, FTX_PAYLOAD_LENGTH_BYTES);
        ft8_encode(msg.payload, d->tones);
        ftx_message_offsets_t offsets;
        d->rc = ftx_message_decode(&msg, &hash_if, d->text, &offsets);
        ftx_field_t types[FTX_MAX_MESSAGE_FIELDS] = { FTX_FIELD_UNKNOWN, FTX_FIELD_UNKNOWN, FTX_FIELD_UNKNOWN };
        ftx_message_type_t type = ftx_message_get_type(&msg);
        if (type == FTX_MESSAGE_TYPE_STANDARD)
            ftx_message_decode_std(&msg, &hash_if, d->call_to, d->call_de, d->extra, types);
        else if (type == FTX_MESSAGE_TYPE_NONSTD_CALL)
            ftx_message_decode_nonstd(&msg, &hash_if, d->call_to, d->call_de, d->extra, types);
        for (int k = 0; k < FTX_MAX_MESSAGE_FIELDS; ++k)
            d->field_types[k] = (int)types[k];
    }
    free(cands);
    monitor_free(&mon);
    return n_out;
}

int mft8_sizeof(void)
{
    return (int)sizeof(mft8_decode_t);
}

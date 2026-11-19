// Clean-room multi-scale coarse-to-fine optical flow with occlusion confidence (cs_5_0).
cbuffer MotionParams : register(b0)
{
    uint2 FrameSize;
    uint2 FlowSize;
    int SearchRadius;
    uint BlockRadius;
    uint PassMode;     // 0 = coarse pyramid pass, 1 = fine refinement + occlusion
    uint CoarseStride; // pixel stride for coarse pass
};

Texture2D<float4> PrevFrame : register(t0);
Texture2D<float4> CurrFrame : register(t1);
Texture2D<float4> CoarseMotion : register(t2);
Texture2D<float2> LumaPair : register(t3);
SamplerState LinearClamp : register(s0);
RWTexture2D<float2> MotionVectors : register(u0);
RWTexture2D<float> OcclusionMask : register(u1);
RWTexture2D<float4> CoarseOutput : register(u2);

#define MAX_SEARCH_RADIUS 16
#define MAX_BLOCK_RADIUS 2
#define CACHE_MAX_DIM 96
#define FLOW_GROUP_SIZE 8

// Pack 2x fp16 luma values per uint (~18.4 KB for 96x96) for higher SM occupancy while still caching 50/75/100% flow scales.
groupshared uint CurrentTile[(CACHE_MAX_DIM * CACHE_MAX_DIM + 1) / 2];
groupshared int2 WideSeeds[4];
groupshared int2 WideOffsets[FLOW_GROUP_SIZE * FLOW_GROUP_SIZE];
groupshared float WideCosts[FLOW_GROUP_SIZE * FLOW_GROUP_SIZE];
groupshared int2 GroupIntMv[FLOW_GROUP_SIZE][FLOW_GROUP_SIZE];

struct CoarseRef
{
    float4 row0; // r00, r10, r20, r01
    float4 row1; // r11, r21, r02, r12
    float r22;
};

float Luma(float3 rgb)
{
    return dot(rgb, float3(0.2126f, 0.7152f, 0.0722f));
}

float PrevAt(int2 position, bool isCoarse)
{
    float value = 0.0f;
    if (isCoarse)
    {
        float2 uv = (float2(position) + 1.0f) / float2(FrameSize);
        value = Luma(PrevFrame.SampleLevel(LinearClamp, saturate(uv), 0).rgb);
    }
    else
    {
        position = clamp(position, int2(0, 0), int2(FrameSize) - 1);
        value = Luma(PrevFrame.Load(int3(position, 0)).rgb);
    }
    return value;
}

float CurrentAtFine(int2 position, bool cached, int2 tileMin, uint tileWidth, uint tileHeight, bool tileCovered)
{
    float value = 0.0f;
    bool hit = false;
    if (cached)
    {
        int2 local = position - tileMin;
        if (tileCovered || (local.x >= 0 && local.y >= 0 && (uint)local.x < tileWidth && (uint)local.y < tileHeight))
        {
            uint idx = (uint)local.y * tileWidth + (uint)local.x;
            uint packed = CurrentTile[idx >> 1];
            value = f16tof32((packed >> ((idx & 1u) * 16u)) & 0xFFFFu);
            hit = true;
        }
    }
    if (!hit)
    {
        position = clamp(position, int2(0, 0), int2(FrameSize) - 1);
        value = Luma(CurrFrame.Load(int3(position, 0)).rgb);
    }
    return value;
}

float CurrentAtCoarse(int2 position)
{
    float2 uv = (float2(position) + 1.0f) / float2(FrameSize);
    return Luma(CurrFrame.SampleLevel(LinearClamp, saturate(uv), 0).rgb);
}

CoarseRef LoadCoarseRef(int2 center, int blockRadius, int patchStep, bool isCoarse)
{
    CoarseRef ref;
    int step = blockRadius * patchStep;
    ref.row0.x = PrevAt(center + int2(-step, -step), isCoarse);
    ref.row0.y = PrevAt(center + int2(0, -step), isCoarse);
    ref.row0.z = PrevAt(center + int2(step, -step), isCoarse);
    ref.row0.w = PrevAt(center + int2(-step, 0), isCoarse);
    ref.row1.x = PrevAt(center, isCoarse);
    ref.row1.y = PrevAt(center + int2(step, 0), isCoarse);
    ref.row1.z = PrevAt(center + int2(-step, step), isCoarse);
    ref.row1.w = PrevAt(center + int2(0, step), isCoarse);
    ref.r22 = PrevAt(center + int2(step, step), isCoarse);
    return ref;
}

float CoarseSad(CoarseRef ref, int2 center, int2 offset, int blockRadius, int patchStep, bool isCoarse, bool cached, int2 tileMin, uint tileWidth, uint tileHeight, bool tileCovered = false)
{
    int step = blockRadius * patchStep;
    int2 c = center + offset;
    float sad = 0.0f;
    if (isCoarse)
    {
        sad += abs(ref.row0.x - CurrentAtCoarse(c + int2(-step, -step)));
        sad += abs(ref.row0.y - CurrentAtCoarse(c + int2(0, -step)));
        sad += abs(ref.row0.z - CurrentAtCoarse(c + int2(step, -step)));
        sad += abs(ref.row0.w - CurrentAtCoarse(c + int2(-step, 0)));
        sad += abs(ref.row1.x - CurrentAtCoarse(c));
        sad += abs(ref.row1.y - CurrentAtCoarse(c + int2(step, 0)));
        sad += abs(ref.row1.z - CurrentAtCoarse(c + int2(-step, step)));
        sad += abs(ref.row1.w - CurrentAtCoarse(c + int2(0, step)));
        sad += abs(ref.r22 - CurrentAtCoarse(c + int2(step, step)));
    }
    else
    {
        sad += abs(ref.row0.x - CurrentAtFine(c + int2(-step, -step), cached, tileMin, tileWidth, tileHeight, tileCovered));
        sad += abs(ref.row0.y - CurrentAtFine(c + int2(0, -step), cached, tileMin, tileWidth, tileHeight, tileCovered));
        sad += abs(ref.row0.z - CurrentAtFine(c + int2(step, -step), cached, tileMin, tileWidth, tileHeight, tileCovered));
        sad += abs(ref.row0.w - CurrentAtFine(c + int2(-step, 0), cached, tileMin, tileWidth, tileHeight, tileCovered));
        sad += abs(ref.row1.x - CurrentAtFine(c, cached, tileMin, tileWidth, tileHeight, tileCovered));
        sad += abs(ref.row1.y - CurrentAtFine(c + int2(step, 0), cached, tileMin, tileWidth, tileHeight, tileCovered));
        sad += abs(ref.row1.z - CurrentAtFine(c + int2(-step, step), cached, tileMin, tileWidth, tileHeight, tileCovered));
        sad += abs(ref.row1.w - CurrentAtFine(c + int2(0, step), cached, tileMin, tileWidth, tileHeight, tileCovered));
        sad += abs(ref.r22 - CurrentAtFine(c + int2(step, step), cached, tileMin, tileWidth, tileHeight, tileCovered));
    }
    return sad;
}

void PushCandidate(
    int2 offset,
    float sad,
    inout int2 cand0,
    inout float sad0,
    inout int2 cand1,
    inout float sad1)
{
    int2 d0 = offset - cand0;
    bool distinct0 = (dot(d0, d0) >= 4);
    if (sad < sad0 || (sad == sad0 && dot(offset, offset) < dot(cand0, cand0)))
    {
        if (distinct0)
        {
            cand1 = cand0;
            sad1 = sad0;
        }
        cand0 = offset;
        sad0 = sad;
    }
    else if (distinct0)
    {
        if (sad < sad1 || (sad == sad1 && dot(offset, offset) < dot(cand1, cand1)))
        {
            cand1 = offset;
            sad1 = sad;
        }
    }
}

float WideSad(int2 base, int2 offset, int2 dim, float prevTaps[9], float bestSad)
{
    float sad = 0.0f;
    int validTaps = 0;
    [unroll] for (int y = -1; y <= 1; ++y)
    {
        [unroll] for (int x = -1; x <= 1; ++x)
        {
            int2 p = base + int2(x, y) * 2;
            int2 q = p + offset;
            if (any(p < 0) || any(q < 0) || any(p >= dim) || any(q >= dim)) continue;
            sad += abs(prevTaps[(y + 1) * 3 + x + 1] - LumaPair.Load(int3(q, 0)).y);
            validTaps += 1;
        }
        // Edge normalization only raises the partial SAD.
        if (y < 1 && sad > bestSad) return 1e30f;
    }
    return validTaps >= 4 ? sad * (9.0f / float(validTaps)) : 1e30f;
}

void Estimate(uint3 tid, uint3 groupId, uint3 groupThreadId, uint mode)
{
    int radius = clamp(SearchRadius, 1, MAX_SEARCH_RADIUS);
    int blockRadius = clamp((int)BlockRadius, 1, MAX_BLOCK_RADIUS);
    uint blockWidth = (uint)(blockRadius * 2 + 1);
    uint lane = groupThreadId.y * FLOW_GROUP_SIZE + groupThreadId.x;
    bool valid = tid.x < FlowSize.x && tid.y < FlowSize.y;

    bool isCoarse = (mode == 0u);
    bool usePyramidSeed = (mode == 1u);
    int stride = isCoarse ? max(1, (int)CoarseStride) : 1;
    int patchStep = isCoarse ? max(1, stride >> 1) : 1;
    int searchRad = (mode == 1u) ? min(radius, 8) : radius;

    uint2 firstFlow = groupId.xy * FLOW_GROUP_SIZE;
    uint2 lastFlow = min(firstFlow + FLOW_GROUP_SIZE - 1, FlowSize - 1);
    int2 firstCenter = int2((float2(firstFlow) + 0.5f) * float2(FrameSize) / float2(FlowSize));
    int2 lastCenter = int2((float2(lastFlow) + 0.5f) * float2(FrameSize) / float2(FlowSize));
    int margin = searchRad + blockRadius + 1;
    int2 tileMin = firstCenter - margin;
    int2 tileMax = lastCenter + margin;
    uint tileWidth = (uint)(tileMax.x - tileMin.x + 1);
    uint tileHeight = (uint)(tileMax.y - tileMin.y + 1);
    bool cached = (!isCoarse) && tileWidth <= CACHE_MAX_DIM && tileHeight <= CACHE_MAX_DIM;

    if (cached)
    {
        uint tileCount = tileWidth * tileHeight;
        uint packedCount = (tileCount + 1u) >> 1;
        for (uint p = lane; p < packedCount; p += FLOW_GROUP_SIZE * FLOW_GROUP_SIZE)
        {
            uint i0 = p << 1;
            int2 pos0 = clamp(tileMin + int2(i0 % tileWidth, i0 / tileWidth), int2(0, 0), int2(FrameSize) - 1);
            float l0 = Luma(CurrFrame.Load(int3(pos0, 0)).rgb);
            float l1 = l0;
            uint i1 = i0 + 1u;
            if (i1 < tileCount)
            {
                int2 pos1 = clamp(tileMin + int2(i1 % tileWidth, i1 / tileWidth), int2(0, 0), int2(FrameSize) - 1);
                l1 = Luma(CurrFrame.Load(int3(pos1, 0)).rgb);
            }
            CurrentTile[p] = f32tof16(l0) | (f32tof16(l1) << 16);
        }
    }

    if (isCoarse)
    {
        // A four-by-four lane tile searches one wide seed. Split the exhaustive grid
        // across its lanes and preserve row-major ties when reducing their winners.
        uint2 local = groupThreadId.xy & 3u;
        uint2 seedThread = groupThreadId.xy - local;
        uint2 seedCell = firstFlow + seedThread;
        uint seedLane = seedThread.y * FLOW_GROUP_SIZE + seedThread.x;
        uint laneInSeed = local.y * 4u + local.x;
        int2 wideBase = int2(min(float2(seedCell) + 2.0f, float2(FlowSize) - 0.5f)
            * float2(FrameSize) / float2(FlowSize)) >> 2;
        uint2 lumaSize;
        LumaPair.GetDimensions(lumaSize.x, lumaSize.y);
        float prevTaps[9];
        [unroll] for (int y = -1; y <= 1; ++y)
            [unroll] for (int x = -1; x <= 1; ++x)
            {
                int2 tap = clamp(wideBase + int2(x, y) * 2, int2(0, 0), int2(lumaSize) - 1);
                prevTaps[(y + 1) * 3 + x + 1] = LumaPair.Load(int3(tap, 0)).x;
            }
        int2 wideBest = int2(0, 0);
        float wideBestSad = 1e30f;
        if (all(seedCell < FlowSize))
        {
            [loop] for (uint candidate = laneInSeed; candidate < 35u * 35u; candidate += 16u)
            {
                int2 offset = int2(candidate % 35u, candidate / 35u) - 17;
                float sad = WideSad(wideBase, offset, int2(lumaSize), prevTaps, wideBestSad);
                if (sad < wideBestSad || (sad == wideBestSad && dot(offset, offset) < dot(wideBest, wideBest)))
                {
                    wideBestSad = sad;
                    wideBest = offset;
                }
            }
        }
        WideOffsets[lane] = wideBest;
        WideCosts[lane] = wideBestSad;
        GroupMemoryBarrierWithGroupSync();

        if (laneInSeed == 0 && valid)
        {
            [loop] for (uint i = 1; i < 16; ++i)
            {
                uint other = seedLane + (i / 4u) * FLOW_GROUP_SIZE + (i % 4u);
                int2 offset = WideOffsets[other];
                float sad = WideCosts[other];
                int distance = dot(offset, offset);
                int bestDistance = dot(wideBest, wideBest);
                bool earlier = offset.y < wideBest.y || (offset.y == wideBest.y && offset.x < wideBest.x);
                if (sad < wideBestSad || (sad == wideBestSad
                    && (distance < bestDistance || (distance == bestDistance && earlier))))
                {
                    wideBestSad = sad;
                    wideBest = offset;
                }
            }
            int2 wideRefined = wideBest;
            if (any(abs(wideBest) == 17))
            {
                for (int ry = -1; ry <= 1; ++ry)
                    for (int rx = -1; rx <= 1; ++rx)
                    {
                        int2 offset = wideBest + int2(rx, ry);
                        if (all(abs(offset) <= 17))
                            continue;
                        float sad = WideSad(wideBase, offset, int2(lumaSize), prevTaps, wideBestSad);
                        if (sad < wideBestSad || (sad == wideBestSad && dot(offset, offset) < dot(wideRefined, wideRefined)))
                        {
                            wideBestSad = sad;
                            wideRefined = offset;
                        }
                    }
            }
            WideSeeds[(seedThread.y >> 2) * 2 + (seedThread.x >> 2)] = wideRefined * 4;
        }
    }
    GroupMemoryBarrierWithGroupSync();

    if (isCoarse)
    {
        if (!valid)
            return;

        int2 center = int2((float2(tid.xy) + 0.5f) * float2(FrameSize) / float2(FlowSize));
        CoarseRef ref = LoadCoarseRef(center, blockRadius, patchStep, true);
        float bestCoarseSad = CoarseSad(ref, center, int2(0, 0), blockRadius, patchStep, true, false, tileMin, tileWidth, tileHeight);
        int2 bestOffset = int2(0, 0);
        for (int cy = -3; cy <= 3; ++cy)
            for (int cx = -3; cx <= 3; ++cx)
            {
                int2 offset = int2(cx, cy) * 7;
                float sad = CoarseSad(ref, center, offset, blockRadius, patchStep, true, false, tileMin, tileWidth, tileHeight);
                if (sad < bestCoarseSad) { bestCoarseSad = sad; bestOffset = offset; }
            }
        int2 coarseBest = bestOffset;
        float bestSubSad = bestCoarseSad;
        for (int dy = -1; dy <= 1; ++dy)
            for (int dx = -1; dx <= 1; ++dx)
            {
                int2 offset = bestOffset + int2(dx, dy) * 2;
                float sad = CoarseSad(ref, center, offset, blockRadius, patchStep, true, false, tileMin, tileWidth, tileHeight);
                if (sad < bestSubSad) { bestSubSad = sad; coarseBest = offset; }
            }
        int2 wideRefined = WideSeeds[(groupThreadId.y >> 2) * 2 + (groupThreadId.x >> 2)];
        CoarseOutput[tid.xy] = float4(coarseBest, wideRefined);
    }
    else
    {
        int2 center = int2(0, 0);
        int2 fineOffset = int2(0, 0);
        float bestFineSad = 1e30f;
        CoarseRef ref;
        ref.row0 = float4(0.0f, 0.0f, 0.0f, 0.0f);
        ref.row1 = float4(0.0f, 0.0f, 0.0f, 0.0f);
        ref.r22 = 0.0f;

        if (valid)
        {
            center = int2((float2(tid.xy) + 0.5f) * float2(FrameSize) / float2(FlowSize));
            ref = LoadCoarseRef(center, blockRadius, patchStep, false);

            float zeroSad = CoarseSad(ref, center, int2(0, 0), blockRadius, patchStep, false, cached, tileMin, tileWidth, tileHeight);
            int2 cand0 = int2(0, 0);
            float sad0 = zeroSad;
            int2 cand1 = int2(0, 0);
            float sad1 = 1e30f;

            // The local search and its patch taps fit inside the group's padded tile.
            [loop] for (int y = -searchRad; y <= searchRad; y += 2)
            {
                [loop] for (int x = -searchRad; x <= searchRad; x += 2)
                {
                    int2 offset = int2(x, y);
                    float sad = CoarseSad(ref, center, offset, blockRadius, patchStep, false, cached, tileMin, tileWidth, tileHeight, true);
                    PushCandidate(offset, sad, cand0, sad0, cand1, sad1);
                }
            }

            if (usePyramidSeed)
            {
                uint2 coarseDim;
                CoarseMotion.GetDimensions(coarseDim.x, coarseDim.y);
                int2 coarseCoord = int2(min(tid.xy >> 1, coarseDim - 1u));
                float4 coarseMv = CoarseMotion.Load(int3(coarseCoord, 0));
                int2 coarseSeed = int2(round(coarseMv.xy));
                int2 wideSeed = int2(round(coarseMv.zw));
                int farGate = max(0, searchRad - 2);

                if (any(abs(coarseSeed) > farGate))
                {
                    for (int sy = -1; sy <= 1; ++sy)
                    {
                        for (int sx = -1; sx <= 1; ++sx)
                        {
                            int2 offset = coarseSeed + int2(sx, sy) * 2;
                            float sad = CoarseSad(ref, center, offset, blockRadius, patchStep, false, cached, tileMin, tileWidth, tileHeight);
                            PushCandidate(offset, sad, cand0, sad0, cand1, sad1);
                        }
                    }
                }

                if (any(abs(wideSeed) > farGate))
                {
                    bool sameAsCoarse = all(wideSeed == coarseSeed) && any(abs(coarseSeed) > farGate);
                    for (int sy = -2; sy <= 2; ++sy)
                    {
                        for (int sx = -2; sx <= 2; ++sx)
                        {
                            if (sameAsCoarse && abs(sx) <= 1 && abs(sy) <= 1)
                                continue;
                            int2 offset = wideSeed + int2(sx, sy) * 2;
                            float sad = CoarseSad(ref, center, offset, blockRadius, patchStep, false, cached, tileMin, tileWidth, tileHeight);
                            PushCandidate(offset, sad, cand0, sad0, cand1, sad1);
                        }
                    }
                }

                // Probe 4 cardinal neighbor CoarseMotion cells to recover from isolated superblock outliers.
                int2 nBestSeed = int2(0, 0);
                float nBestSad = 1e30f;
                float sadBeforeNeighbor = sad0;
                const int2 nDirs[4] = { int2(-1, 0), int2(1, 0), int2(0, -1), int2(0, 1) };
                [unroll] for (int ni = 0; ni < 4; ++ni)
                {
                    int2 nCoord = clamp(coarseCoord + nDirs[ni], int2(0, 0), int2(coarseDim) - 1);
                    float4 nMv = CoarseMotion.Load(int3(nCoord, 0));
                    int2 nCoarse = int2(round(nMv.xy));
                    int2 nWide = int2(round(nMv.zw));
                    if (any(nCoarse != coarseSeed) && any(nCoarse != wideSeed) && any(abs(nCoarse) > farGate))
                    {
                        float sad = CoarseSad(ref, center, nCoarse, blockRadius, patchStep, false, cached, tileMin, tileWidth, tileHeight);
                        PushCandidate(nCoarse, sad, cand0, sad0, cand1, sad1);
                        if (sad < nBestSad)
                        {
                            nBestSad = sad;
                            nBestSeed = nCoarse;
                        }
                    }
                    if (any(nWide != wideSeed) && any(nWide != coarseSeed) && any(nWide != nCoarse) && any(abs(nWide) > farGate))
                    {
                        float sad = CoarseSad(ref, center, nWide, blockRadius, patchStep, false, cached, tileMin, tileWidth, tileHeight);
                        PushCandidate(nWide, sad, cand0, sad0, cand1, sad1);
                        if (sad < nBestSad)
                        {
                            nBestSad = sad;
                            nBestSeed = nWide;
                        }
                    }
                }
                if (nBestSad < sadBeforeNeighbor)
                {
                    for (int sy = -1; sy <= 1; ++sy)
                    {
                        for (int sx = -1; sx <= 1; ++sx)
                        {
                            int2 offset = nBestSeed + int2(sx, sy) * 2;
                            float sad = CoarseSad(ref, center, offset, blockRadius, patchStep, false, cached, tileMin, tileWidth, tileHeight);
                            PushCandidate(offset, sad, cand0, sad0, cand1, sad1);
                        }
                    }
                }
            }

            // Refine cand0 (and competitive cand1 in Balanced) with 5x5-span CoarseSad reusing register-cached Prev.
            fineOffset = cand0;
            for (int dy = -1; dy <= 1; ++dy)
            {
                for (int dx = -1; dx <= 1; ++dx)
                {
                    int2 offset = cand0 + int2(dx, dy);
                    float sad = CoarseSad(ref, center, offset, blockRadius, 1, false, cached, tileMin, tileWidth, tileHeight);
                    if (sad < bestFineSad || (sad == bestFineSad && dot(offset, offset) < dot(fineOffset, fineOffset)))
                    {
                        bestFineSad = sad;
                        fineOffset = offset;
                    }
                }
            }
            if (radius > 5 && sad1 < sad0 * 1.15f + 0.05f)
            {
                for (int dy = -1; dy <= 1; ++dy)
                {
                    for (int dx = -1; dx <= 1; ++dx)
                    {
                        int2 offset = cand1 + int2(dx, dy);
                        if (all(abs(offset - cand0) <= 1))
                            continue;
                        float sad = CoarseSad(ref, center, offset, blockRadius, 1, false, cached, tileMin, tileWidth, tileHeight);
                        if (sad < bestFineSad || (sad == bestFineSad && dot(offset, offset) < dot(fineOffset, fineOffset)))
                        {
                            bestFineSad = sad;
                            fineOffset = offset;
                        }
                    }
                }
            }
            // Step-two candidates can both follow the aperture ambiguity past an odd-pixel match.
            int2 anchors[3] = { int2(0, 0), int2(0, 0), int2(0, 0) };
            if (usePyramidSeed)
            {
                uint2 cd;
                CoarseMotion.GetDimensions(cd.x, cd.y);
                float4 seeds = CoarseMotion.Load(int3(min(tid.xy >> 1, cd - 1u), 0));
                anchors[0] = int2(round(seeds.xy));
                anchors[1] = int2(round(seeds.zw));
            }
            [loop] for (int ai = 0; ai < 3; ++ai)
            {
                if ((ai == 1 && all(anchors[1] == anchors[0])) || (ai == 2 && (all(anchors[0] == 0) || all(anchors[1] == 0)))) continue;
                [unroll] for (int ay = -1; ay <= 1; ++ay)
                    [unroll] for (int ax = -1; ax <= 1; ++ax)
                    {
                        int2 offset = anchors[ai] + int2(ax, ay);
                        if (all(abs(offset - cand0) <= 1) || (radius > 5 && sad1 < sad0 * 1.15f + 0.05f && all(abs(offset - cand1) <= 1))) continue;
                        float sad = CoarseSad(ref, center, offset, blockRadius, 1, false, cached, tileMin, tileWidth, tileHeight);
                        if (sad < bestFineSad || (sad == bestFineSad && dot(offset,offset) < dot(fineOffset,fineOffset)))
                        {
                            bestFineSad = sad;
                            fineOffset = offset;
                        }
                    }
            }
        }

        GroupIntMv[groupThreadId.y][groupThreadId.x] = fineOffset;
        GroupMemoryBarrierWithGroupSync();

        if (!valid)
            return;

        // In-group 4-neighbor spatial MV consensus before subpixel fit to suppress isolated outliers.
        const int2 gDirs[4] = { int2(-1, 0), int2(1, 0), int2(0, -1), int2(0, 1) };
        int2 gMvs[4];
        bool gValid[4];
        [unroll] for (int gi = 0; gi < 4; ++gi)
        {
            int2 gp = int2(groupThreadId.xy) + gDirs[gi];
            int2 tp = int2(tid.xy) + gDirs[gi];
            gValid[gi] = (gp.x >= 0 && gp.y >= 0 && gp.x < FLOW_GROUP_SIZE && gp.y < FLOW_GROUP_SIZE
                && tp.x >= 0 && tp.y >= 0 && tp.x < (int)FlowSize.x && tp.y < (int)FlowSize.y);
            gMvs[gi] = gValid[gi] ? GroupIntMv[gp.y][gp.x] : fineOffset;
        }

        int2 consensusMv = fineOffset;
        float consensusSad = bestFineSad;
        [loop] for (int ci = 0; ci < 4; ++ci)
        {
            if (!gValid[ci])
                continue;
            int2 candMv = gMvs[ci];
            if (all(candMv == fineOffset))
                continue;

            bool seen = false;
            [unroll] for (int pj = 0; pj < 4; ++pj)
            {
                if (pj < ci && gValid[pj] && all(gMvs[pj] == candMv))
                    seen = true;
            }
            if (seen)
                continue;

            int agreeCount = 0;
            [unroll] for (int aj = 0; aj < 4; ++aj)
            {
                if (gValid[aj])
                {
                    int2 diffMv = abs(gMvs[aj] - candMv);
                    if (diffMv.x <= 1 && diffMv.y <= 1)
                        agreeCount += 1;
                }
            }

            float nSad = CoarseSad(ref, center, candMv, blockRadius, 1, false, cached, tileMin, tileWidth, tileHeight);
            bool better = (nSad < consensusSad);
            bool tieBreakConsensus = (bestFineSad > 0.005f && agreeCount >= 2 && nSad <= bestFineSad * 1.03f + 0.02f && nSad <= consensusSad);
            if (better || tieBreakConsensus)
            {
                consensusSad = nSad;
                consensusMv = candMv;
            }
        }
        fineOffset = consensusMv;
        bestFineSad = consensusSad;

        float centerSubSad = bestFineSad;
        float leftSad = CoarseSad(ref, center, fineOffset + int2(-1, 0), blockRadius, 1, false, cached, tileMin, tileWidth, tileHeight);
        float rightSad = CoarseSad(ref, center, fineOffset + int2(1, 0), blockRadius, 1, false, cached, tileMin, tileWidth, tileHeight);
        float upSad = CoarseSad(ref, center, fineOffset + int2(0, -1), blockRadius, 1, false, cached, tileMin, tileWidth, tileHeight);
        float downSad = CoarseSad(ref, center, fineOffset + int2(0, 1), blockRadius, 1, false, cached, tileMin, tileWidth, tileHeight);
        float denomX = leftSad - 2.0f * centerSubSad + rightSad;
        float denomY = upSad - 2.0f * centerSubSad + downSad;
        float subX = (denomX > 1e-5f) ? clamp(0.5f * (leftSad - rightSad) / denomX, -0.5f, 0.5f) : 0.0f;
        float subY = (denomY > 1e-5f) ? clamp(0.5f * (upSad - downSad) / denomY, -0.5f, 0.5f) : 0.0f;
        float2 subOffset = (centerSubSad < 0.025f) ? float2(0.0f, 0.0f) : float2(subX, subY);
        float2 finalMv = float2(fineOffset) + subOffset;

        float normSad = centerSubSad / 9.0f;
        float sadConf = 1.0f - smoothstep(0.10f, 0.35f, normSad);
        MotionVectors[tid.xy] = finalMv;
        OcclusionMask[tid.xy] = sadConf;
    }
}


[numthreads(FLOW_GROUP_SIZE, FLOW_GROUP_SIZE, 1)]
void main(uint3 tid : SV_DispatchThreadID, uint3 groupId : SV_GroupID, uint3 groupThreadId : SV_GroupThreadID)
{
    Estimate(tid, groupId, groupThreadId, PassMode);
}

[numthreads(FLOW_GROUP_SIZE, FLOW_GROUP_SIZE, 1)]
void coarse(uint3 tid : SV_DispatchThreadID, uint3 groupId : SV_GroupID, uint3 groupThreadId : SV_GroupThreadID)
{
    Estimate(tid, groupId, groupThreadId, 0u);
}

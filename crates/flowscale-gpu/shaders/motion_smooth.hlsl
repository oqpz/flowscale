// Spatial 3x3 outlier suppression & coherence filtering for optical flow (cs_5_0).
cbuffer MotionParams : register(b0)
{
    uint2 FrameSize;
    uint2 FlowSize;
    int SearchRadius;
    uint BlockRadius;
    uint PassMode;
    uint CoarseStride;
};

Texture2D<float2> RawMotion : register(t0);
Texture2D<float> RawOcclusion : register(t1);
Texture2D<float4> PrevFrame : register(t2);
Texture2D<float4> CurrFrame : register(t3);
RWTexture2D<float2> OutMotion : register(u0);
RWTexture2D<float> OutOcclusion : register(u1);
SamplerState LinearClamp : register(s0);

float Luma(float3 rgb)
{
    return dot(rgb, float3(0.2126f, 0.7152f, 0.0722f));
}

float ForwardSad5(float2 uv, float2 mvPx, float2 invFrame)
{
    float2 uvCurr = uv + mvPx * invFrame;
    const float2 taps[5] = {
        float2(0.0f, 0.0f),
        float2(-2.0f, 0.0f),
        float2(2.0f, 0.0f),
        float2(0.0f, -2.0f),
        float2(0.0f, 2.0f)
    };
    float sad = 0.0f;
    [unroll] for (int i = 0; i < 5; ++i)
    {
        float2 d = taps[i] * invFrame;
        float lp = Luma(PrevFrame.SampleLevel(LinearClamp, saturate(uv + d), 0).rgb);
        float lc = Luma(CurrFrame.SampleLevel(LinearClamp, saturate(uvCurr + d), 0).rgb);
        sad += abs(lp - lc);
    }
    return sad;
}

bool PatchAgrees(float2 uv, float2 motion, float2 texel)
{
    float2 q = uv + motion * texel;
    if (any(q < 2.5f * texel) || any(q > 1.0f - 2.5f * texel))
        return false;
    const float2 taps[4] = { float2(-2,0), float2(2,0), float2(0,-2), float2(0,2) };
    float p0 = Luma(PrevFrame.SampleLevel(LinearClamp, uv, 0).rgb);
    float q0 = Luma(CurrFrame.SampleLevel(LinearClamp, q, 0).rgb);
    float dotPQ = 0.0f, dotPP = 0.0f, dotQQ = 0.0f;
    [unroll] for (int i = 0; i < 4; ++i)
    {
        float p = Luma(PrevFrame.SampleLevel(LinearClamp, uv + taps[i] * texel, 0).rgb) - p0;
        float c = Luma(CurrFrame.SampleLevel(LinearClamp, q + taps[i] * texel, 0).rgb) - q0;
        dotPQ += p * c;
        dotPP += p * p;
        dotQQ += c * c;
    }
    return dotPP > 0.00005f && dotQQ > 0.00005f && dotPQ > 0.0f
        && dotPQ * dotPQ > 0.90f * dotPP * dotQQ
        && max(dotPP, dotQQ) <= 9.0f * min(dotPP, dotQQ);
}

[numthreads(8, 8, 1)]
void main(uint3 tid : SV_DispatchThreadID)
{
    if (tid.x >= FlowSize.x || tid.y >= FlowSize.y)
        return;

    float2 mvs[9];
    bool valid[9];
    int validCount = 0;
    [unroll] for (int dy = -1; dy <= 1; ++dy)
    {
        [unroll] for (int dx = -1; dx <= 1; ++dx)
        {
            int idx = (dy + 1) * 3 + (dx + 1);
            int2 p = int2(tid.xy) + int2(dx, dy);
            bool inBounds = (p.x >= 0 && p.y >= 0 && p.x < (int)FlowSize.x && p.y < (int)FlowSize.y);
            valid[idx] = inBounds;
            mvs[idx] = inBounds ? RawMotion.Load(int3(p, 0)) : float2(0.0f, 0.0f);
            validCount += inBounds ? 1 : 0;
        }
    }

    float2 centerMv = mvs[4];
    int centerSupport = 0;
    [unroll] for (int j = 0; j < 9; ++j)
    {
        if (valid[j])
        {
            float2 diff = mvs[j] - centerMv;
            if (dot(diff, diff) <= 4.0f)
                centerSupport += 1;
        }
    }

    float2 candMv = centerMv;
    int candSupport = centerSupport;
    [unroll] for (int i = 0; i < 9; ++i)
    {
        if (i == 4 || !valid[i])
            continue;
        float2 candidate = mvs[i];
        int sup = 0;
        [unroll] for (int k = 0; k < 9; ++k)
        {
            if (valid[k])
            {
                float2 diff = mvs[k] - candidate;
                if (dot(diff, diff) <= 4.0f)
                    sup += 1;
            }
        }
        if (sup > candSupport)
        {
            candSupport = sup;
            candMv = candidate;
        }
    }

    float2 chosenMv = centerMv;
    int chosenSupport = centerSupport;
    float2 delta = centerMv - candMv;
    bool isOutlier = (centerSupport < candSupport)
        && (candSupport * 9 >= 5 * validCount)
        && (dot(delta, delta) > 6.25f);
    if (isOutlier)
    {
        float2 uv = (float2(tid.xy) + 0.5f) / float2(FlowSize);
        float2 invFrame = 1.0f / float2(FrameSize);
        float centerSad = ForwardSad5(uv, centerMv, invFrame);
        float candSad = ForwardSad5(uv, candMv, invFrame);
        if (candSad < centerSad * 0.85f + 0.002f)
        {
            chosenMv = candMv;
            chosenSupport = candSupport;
        }
    }

    float scaledSupport = (validCount > 0) ? (float(chosenSupport) * 9.0f / float(validCount)) : float(chosenSupport);
    float coherence = saturate((scaledSupport - 2.0f) / 5.0f);
    // A strong camera translation can rescue isolated vectors on translucent
    // regions, where contrast changes but the background texture still aligns.
    if (scaledSupport < 6.0f)
    {
        uint2 mid = FlowSize / 2;
        float2 pan = RawMotion.Load(int3(mid, 0));
        if (dot(pan, pan) >= 400.0f && length(pan - chosenMv) > 2.0f)
        {
            const float2 positions[4] = { float2(0.25f,0.5f), float2(0.75f,0.5f), float2(0.5f,0.25f), float2(0.5f,0.75f) };
            bool agrees = true;
            [unroll] for (int k = 0; k < 4; ++k)
            {
                float2 offset = RawMotion.Load(int3(int2(float2(FlowSize) * positions[k]), 0)) - pan;
                agrees = agrees && dot(offset, offset) <= 1.0f;
            }
            float2 uv = (float2(tid.xy) + 0.5f) / float2(FlowSize);
            if (agrees && PatchAgrees(uv, pan, 1.0f / float2(FrameSize)))
            {
                chosenMv = pan;
                coherence = 1.0f;
            }
        }
    }
    OutMotion[tid.xy] = chosenMv;
    OutOcclusion[tid.xy] = saturate(RawOcclusion.Load(int3(tid.xy, 0))) * lerp(0.88f, 1.0f, coherence);
}

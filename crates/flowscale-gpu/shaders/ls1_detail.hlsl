// Clean-room multi-band edge-preserving upscaler (cs_5_0).
// Subtype: 0 = Balanced (4x4 base + multi-band detail), 1 = Performance (bilinear + 5-tap detail).
// Sharpness: 0..4 (clamped safely for broader inputs).
cbuffer UpscaleParams : register(b0)
{
    uint2 InputSize;
    uint2 OutputSize;
    uint4 DstRect;
    float Sharpness;
    uint ModeFlags;
    uint Subtype;
    uint VrsFlag;
};

Texture2D<float4> InputTex : register(t0);
RWTexture2D<float4> OutputTex : register(u0);
SamplerState LinearClamp : register(s0);

float Luma(float3 c)
{
    return dot(c, float3(0.2126f, 0.7152f, 0.0722f));
}

float CubicWeight(float x)
{
    float ax = abs(x);
    if (ax <= 1.0f)
        return (1.5f * ax - 2.5f) * ax * ax + 1.0f;
    if (ax < 2.0f)
        return ((-0.5f * ax + 2.5f) * ax - 4.0f) * ax + 2.0f;
    return 0.0f;
}

[numthreads(8, 8, 1)]
void main(uint3 tid : SV_DispatchThreadID)
{
    if (tid.x >= OutputSize.x || tid.y >= OutputSize.y)
        return;

    if (tid.x < DstRect.x || tid.y < DstRect.y ||
        tid.x >= DstRect.x + DstRect.z || tid.y >= DstRect.y + DstRect.w)
    {
        OutputTex[tid.xy] = float4(0.0f, 0.0f, 0.0f, 1.0f);
        return;
    }

    uint2 localTid = tid.xy - DstRect.xy;
    uint2 dstSize = max(DstRect.zw, uint2(1, 1));

    float2 uv = (float2(localTid) + 0.5f) / float2(dstSize);
    float2 invIn = 1.0f / float2(InputSize);
    float sharpNorm = saturate(Sharpness * 0.25f);

    float4 linearCenter = InputTex.SampleLevel(LinearClamp, uv, 0);

    float3 baseRgb = linearCenter.rgb;
    float3 minC = baseRgb;
    float3 maxC = baseRgb;

    if (Subtype == 0u)
    {
        // Balanced: 4x4 Catmull-Rom reconstruction with anti-ringing bounds.
        float2 pos = uv * float2(InputSize) - 0.5f;
        int2 baseCoord = int2(floor(pos));
        float2 fracPos = pos - float2(baseCoord);

        float3 accum = 0.0f;
        float wSum = 0.0f;
        minC = 1e9f;
        maxC = -1e9f;

        [unroll]
        for (int j = -1; j <= 2; ++j)
        {
            float wy = CubicWeight(float(j) - fracPos.y);
            int sy = clamp(baseCoord.y + j, 0, int(InputSize.y) - 1);
            [unroll]
            for (int i = -1; i <= 2; ++i)
            {
                float wx = CubicWeight(float(i) - fracPos.x);
                int sx = clamp(baseCoord.x + i, 0, int(InputSize.x) - 1);
                float3 tap = InputTex.Load(int3(sx, sy, 0)).rgb;
                float w = wx * wy;
                accum += tap * w;
                wSum += w;
                if (i >= 0 && i <= 1 && j >= 0 && j <= 1)
                {
                    minC = min(minC, tap);
                    maxC = max(maxC, tap);
                }
            }
        }
        baseRgb = clamp(accum / max(wSum, 1e-5f), minC, maxC);
    }

    // Multi-band detail extraction (fine 1-texel cross + medium 2-texel cross).
    float3 n1 = InputTex.SampleLevel(LinearClamp, uv + float2(0.0f, -invIn.y), 0).rgb;
    float3 s1 = InputTex.SampleLevel(LinearClamp, uv + float2(0.0f,  invIn.y), 0).rgb;
    float3 w1 = InputTex.SampleLevel(LinearClamp, uv + float2(-invIn.x, 0.0f), 0).rgb;
    float3 e1 = InputTex.SampleLevel(LinearClamp, uv + float2( invIn.x, 0.0f), 0).rgb;

    float3 fineBand = baseRgb - 0.25f * (n1 + s1 + w1 + e1);
    float3 detailBoost = fineBand * (0.45f * sharpNorm);

    if (Subtype == 0u)
    {
        float3 n2 = InputTex.SampleLevel(LinearClamp, uv + float2(0.0f, -2.0f * invIn.y), 0).rgb;
        float3 s2 = InputTex.SampleLevel(LinearClamp, uv + float2(0.0f,  2.0f * invIn.y), 0).rgb;
        float3 w2 = InputTex.SampleLevel(LinearClamp, uv + float2(-2.0f * invIn.x, 0.0f), 0).rgb;
        float3 e2 = InputTex.SampleLevel(LinearClamp, uv + float2( 2.0f * invIn.x, 0.0f), 0).rgb;
        float3 medBand = baseRgb - 0.25f * (n2 + s2 + w2 + e2);
        detailBoost += medBand * (0.20f * sharpNorm);
    }

    // Edge-preserving soft-clamp so high-contrast boundaries stay crisp without overshoot.
    float edgeContrast = saturate(abs(Luma(n1) - Luma(s1)) + abs(Luma(w1) - Luma(e1)));
    float gain = lerp(1.0f, 0.55f, edgeContrast);
    float3 finalRgb = baseRgb + detailBoost * gain;
    if (Subtype == 0u)
    {
        finalRgb = clamp(finalRgb, minC - abs(minC) * 0.08f, maxC + abs(maxC) * 0.08f);
    }

    OutputTex[tid.xy] = float4(finalRgb, linearCenter.a);
}

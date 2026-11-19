// Clean-room Catmull-Rom 4x4 bicubic upscaler with local contrast sharpening (cs_5_0).
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

float CatmullRomWeight(float x)
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

    float2 pos = (float2(localTid) + 0.5f) * float2(InputSize) / float2(dstSize) - 0.5f;
    int2 baseCoord = int2(floor(pos));
    float2 fracPos = pos - float2(baseCoord);

    float4 accum = 0.0f;
    float weightSum = 0.0f;
    float3 minNeighborhood = 1e9f;
    float3 maxNeighborhood = -1e9f;

    [unroll]
    for (int j = -1; j <= 2; ++j)
    {
        float wy = CatmullRomWeight(float(j) - fracPos.y);
        int sy = clamp(baseCoord.y + j, 0, int(InputSize.y) - 1);

        [unroll]
        for (int i = -1; i <= 2; ++i)
        {
            float wx = CatmullRomWeight(float(i) - fracPos.x);
            int sx = clamp(baseCoord.x + i, 0, int(InputSize.x) - 1);

            float4 tap = InputTex.Load(int3(sx, sy, 0));
            float w = wx * wy;
            accum += tap * w;
            weightSum += w;

            if (i >= 0 && i <= 1 && j >= 0 && j <= 1)
            {
                minNeighborhood = min(minNeighborhood, tap.rgb);
                maxNeighborhood = max(maxNeighborhood, tap.rgb);
            }
        }
    }

    float4 result = accum / max(weightSum, 1e-5f);
    result.rgb = clamp(result.rgb, minNeighborhood, maxNeighborhood);

    if (Sharpness > 0.001f)
    {
        float3 c = result.rgb;
        float3 mid = 0.5f * (minNeighborhood + maxNeighborhood);
        float contrast = saturate(Sharpness * 0.08f);
        result.rgb = clamp(c + (c - mid) * contrast, minNeighborhood - abs(minNeighborhood) * 0.05f, maxNeighborhood + abs(maxNeighborhood) * 0.05f);
    }

    OutputTex[tid.xy] = float4(result.rgb, result.a);
}
